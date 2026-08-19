use crate::error::GlossError;
use candle_transformers::models::nomic_bert::{
    l2_normalize, mean_pooling, Config as NomicBertConfig, NomicBertModel,
};
use sha2::{Digest, Sha256};
use std::path::Path;
use usearch::ffi::{IndexOptions, MetricKind, ScalarKind};

/// The single native dense artifact. Every native ingestion/rebuild path must
/// use this identifier rather than inventing a second index file.
pub const NATIVE_DENSE_ARTIFACT_FILENAME: &str = "chunks.usearch";

pub fn native_dense_artifact_path(notebook_dir: &Path) -> std::path::PathBuf {
    notebook_dir
        .join("embeddings")
        .join(NATIVE_DENSE_ARTIFACT_FILENAME)
}

/// HNSW vector index using usearch (C++ via FFI, but only for add/search/save —
/// no model inference here, so heap corruption from ONNX batch embed is isolated).
pub struct HnswIndex {
    index: usearch::Index,
    dims: usize,
}

const INITIAL_HNSW_CAPACITY: usize = 1_024;

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
        index
            .reserve(INITIAL_HNSW_CAPACITY)
            .map_err(|e| GlossError::Embedding(format!("Failed to reserve HNSW index: {e}")))?;
        Ok(Self { index, dims })
    }

    pub fn load_with_hwm(path: &Path, hwm: i64, dims: usize) -> Result<Self, GlossError> {
        let index = Self::new(dims)?;
        if path.exists() {
            index
                .index
                .load(path.to_str().unwrap_or(""))
                .map_err(|e| GlossError::Embedding(format!("Failed to load HNSW index: {e}")))?;
            let hwm_capacity = usize::try_from(hwm).unwrap_or(0);
            let capacity = hwm_capacity.max(index.index.size());
            index
                .index
                .reserve(capacity)
                .map_err(|e| GlossError::Embedding(format!("Failed to reserve HNSW index: {e}")))?;
        }
        Ok(index)
    }

    pub fn add(&mut self, vector: &[f32]) -> Result<u64, GlossError> {
        if vector.len() != self.dims {
            return Err(GlossError::Embedding(format!(
                "HNSW add dimension mismatch: vector has {} dims, index expects {}",
                vector.len(),
                self.dims
            )));
        }
        let label = self.index.size() as u64;
        if self.index.size() >= self.index.capacity() {
            let capacity = self
                .index
                .capacity()
                .max(INITIAL_HNSW_CAPACITY)
                .checked_mul(2)
                .ok_or_else(|| GlossError::Embedding("HNSW capacity growth overflow".into()))?;
            self.index.reserve(capacity).map_err(|e| {
                GlossError::Embedding(format!("Failed to grow HNSW index capacity: {e}"))
            })?;
        }
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
        if query.len() != self.dims {
            return Err(GlossError::Embedding(format!(
                "HNSW search dimension mismatch: query has {} dims, index expects {}",
                query.len(),
                self.dims
            )));
        }
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

    /// Publish a dense artifact only after a separate reload verifies the
    /// written bytes. Metadata must be advanced after this method succeeds.
    pub fn save_atomic_verified(&self, path: &Path, hwm: i64) -> Result<(), GlossError> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(NATIVE_DENSE_ARTIFACT_FILENAME);
        let temporary = parent.join(format!(".{filename}.{}.tmp", uuid::Uuid::new_v4()));

        let result = (|| {
            self.save(&temporary)?;
            // Loading the temporary artifact catches corrupt/partial writes
            // before the canonical path is replaced.
            let verified = Self::load_with_hwm(&temporary, hwm, self.dims)?;
            if verified.dims != self.dims {
                return Err(GlossError::Embedding(
                    "HNSW reload verification changed embedding dimensions".to_string(),
                ));
            }
            std::mem::forget(verified);
            std::fs::rename(&temporary, path)?;
            let published = Self::load_with_hwm(path, hwm, self.dims)?;
            std::mem::forget(published);
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }

    #[allow(dead_code)]
    pub fn size(&self) -> usize {
        self.index.size()
    }
}

/// Embedding backend: local CPU candle (Nomic v1.5) or external Ollama.
///
/// The candle path never touches ONNX/ort and uses the CPU unconditionally —
/// it is the automatic fallback whenever no Ollama server is configured or
/// reachable.
pub enum EmbeddingBackend {
    NomicV15(NomicV15Embedder),
    Ollama {
        client: reqwest::Client,
        url: String,
        model: String,
    },
}

/// The canonical local embedding model (candle, CPU, 768 dims).
pub const CANDLE_EMBEDDING_MODEL: &str = "nomic-ai/nomic-embed-text-v1.5";

fn hf_cache_repo_dir(hf_home: &Path) -> std::path::PathBuf {
    hf_home.join("models--nomic-ai--nomic-embed-text-v1.5")
}

/// The hf-hub cache root, mirroring the `hf-hub` crate resolution:
/// `$HF_HOME/hub`, else `$HOME/.cache/huggingface/hub`.
pub fn hf_hub_cache_dir() -> std::path::PathBuf {
    if let Ok(hf_home) = std::env::var("HF_HOME") {
        let trimmed = hf_home.trim();
        if !trimmed.is_empty() {
            return std::path::PathBuf::from(trimmed).join("hub");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home)
        .join(".cache")
        .join("huggingface")
        .join("hub")
}

fn snapshot_has_model_files(snapshot: &Path) -> bool {
    let has_weights = snapshot.join("model.safetensors").exists()
        || std::fs::read_dir(snapshot)
            .map(|entries| {
                entries.flatten().any(|entry| {
                    let name = entry.file_name().to_string_lossy().to_string();
                    name.starts_with("model-") && name.ends_with(".safetensors")
                })
            })
            .unwrap_or(false);
    snapshot.join("config.json").exists() && has_weights && snapshot.join("tokenizer.json").exists()
}

/// True when the local candle model's files are present in either the hf-hub
/// cache or the legacy `data_dir/models` cache. This is what makes the CPU
/// candle fallback work without download consent when the model already exists.
pub fn candle_model_is_cached(hf_home: &Path, legacy_cache: &Path) -> bool {
    [
        hf_cache_repo_dir(hf_home),
        legacy_cache.join("models--nomic-ai--nomic-embed-text-v1.5"),
    ]
    .iter()
    .any(|repo_dir| {
        let snapshots = repo_dir.join("snapshots");
        if !snapshots.is_dir() {
            return false;
        }
        // hf-hub layout: snapshots/<revision>/<files>
        if std::fs::read_dir(&snapshots)
            .map(|entries| {
                entries
                    .flatten()
                    .any(|entry| entry.path().is_dir() && snapshot_has_model_files(&entry.path()))
            })
            .unwrap_or(false)
        {
            return true;
        }
        // Some partial downloaders leave symlinks directly under snapshots/.
        snapshot_has_model_files(&snapshots)
    })
}

/// Repair a missing/empty `refs/main` revision pointer when exactly one usable
/// snapshot revision exists, so hf-hub can resolve the model offline. A broken
/// pointer (e.g. an empty `refs/main`) otherwise forces a network re-fetch even
/// though every model file is already cached.
fn repair_hf_refs_if_needed(repo_dir: &Path) {
    let refs_main = repo_dir.join("refs").join("main");
    let existing = std::fs::read_to_string(&refs_main).unwrap_or_default();
    if !existing.trim().is_empty() {
        return;
    }
    let snapshots = repo_dir.join("snapshots");
    let Ok(entries) = std::fs::read_dir(&snapshots) else {
        return;
    };
    let revisions: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.is_dir() && snapshot_has_model_files(&path) {
                Some(entry.file_name().to_string_lossy().to_string())
            } else {
                None
            }
        })
        .collect();
    if revisions.len() != 1 {
        return;
    }
    let _ = std::fs::create_dir_all(repo_dir.join("refs"));
    if let Err(e) = std::fs::write(&refs_main, &revisions[0]) {
        tracing::warn!(error = %e, "Failed to repair hf-hub refs/main pointer");
    } else {
        tracing::info!(revision = %revisions[0], "Repaired hf-hub refs/main pointer");
    }
}

fn resolve_snapshot_revision(repo_dir: &Path) -> Option<String> {
    repair_hf_refs_if_needed(repo_dir);
    let refs_main = repo_dir.join("refs").join("main");
    let revision = std::fs::read_to_string(&refs_main).ok()?.trim().to_string();
    if revision.is_empty() || !repo_dir.join("snapshots").join(&revision).is_dir() {
        return None;
    }
    Some(revision)
}

/// In-process CPU embedder for `nomic-ai/nomic-embed-text-v1.5` built directly
/// on candle-transformers' nomic_bert implementation (SwiGLU `fc11`/`fc12`
/// MLP, fused QKV, RoPE — the real v1.5 architecture).
///
/// fastembed 5.x's `NomicV2Moe` loader targets the *v2-moe* weight layout
/// (`fc1`/`fc2` + router experts) and cannot load v1.5 at any revision, so
/// this loader replaces it. It reads the standard hf-hub cache and works
/// offline once the model is present.
pub struct NomicV15Embedder {
    model: NomicBertModel,
    tokenizer: tokenizers::Tokenizer,
    dims: usize,
}

impl NomicV15Embedder {
    /// The padding/truncation length used for CPU inference (matches the
    /// previous fastembed configuration).
    pub const MAX_LENGTH: usize = 2048;

    fn load_from_paths(
        config_path: &Path,
        weights_path: &Path,
        tokenizer_path: &Path,
        max_length: usize,
    ) -> Result<Self, GlossError> {
        let config_str = std::fs::read_to_string(config_path)
            .map_err(|e| GlossError::Embedding(format!("failed to read nomic config: {e}")))?;
        let config: NomicBertConfig = serde_json::from_str(&config_str)
            .map_err(|e| GlossError::Embedding(format!("failed to parse nomic config: {e}")))?;
        if config.n_embd != 768 {
            return Err(GlossError::Embedding(format!(
                "unexpected nomic embedding dims {} (expected 768)",
                config.n_embd
            )));
        }

        let device = candle_core::Device::Cpu;
        let weights_bytes = std::fs::read(weights_path)
            .map_err(|e| GlossError::Embedding(format!("failed to read nomic weights: {e}")))?;
        let vb = candle_nn::VarBuilder::from_buffered_safetensors(
            weights_bytes,
            candle_core::DType::F32,
            &device,
        )
        .map_err(|e| GlossError::Embedding(format!("failed to map nomic weights: {e}")))?;
        let model = NomicBertModel::load(vb, &config)
            .map_err(|e| GlossError::Embedding(format!("failed to build nomic model: {e}")))?;

        let mut tokenizer = tokenizers::Tokenizer::from_file(tokenizer_path)
            .map_err(|e| GlossError::Embedding(format!("failed to load nomic tokenizer: {e}")))?;
        let _ = tokenizer.with_padding(Some(tokenizers::PaddingParams {
            strategy: tokenizers::PaddingStrategy::BatchLongest,
            direction: tokenizers::PaddingDirection::Right,
            pad_id: 0,
            pad_token: "<pad>".to_string(),
            ..Default::default()
        }));
        let _ = tokenizer.with_truncation(Some(tokenizers::TruncationParams {
            max_length,
            ..Default::default()
        }));

        Ok(Self {
            model,
            tokenizer,
            dims: config.n_embd,
        })
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, GlossError> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| GlossError::Embedding(format!("nomic tokenization failed: {e}")))?;
        let ids: Vec<Vec<u32>> = encodings.iter().map(|e| e.get_ids().to_vec()).collect();
        let masks: Vec<Vec<u32>> = encodings
            .iter()
            .map(|e| e.get_attention_mask().to_vec())
            .collect();

        let device = self.model.device.clone();
        let input_ids = candle_core::Tensor::new(ids, &device)
            .map_err(|e| GlossError::Embedding(format!("nomic input tensor failed: {e}")))?;
        let attention_mask = candle_core::Tensor::new(masks, &device)
            .map_err(|e| GlossError::Embedding(format!("nomic mask tensor failed: {e}")))?;
        let hidden = self
            .model
            .forward(&input_ids, None, Some(&attention_mask))
            .map_err(|e| GlossError::Embedding(format!("nomic forward failed: {e}")))?;
        let pooled = mean_pooling(&hidden, &attention_mask)
            .map_err(|e| GlossError::Embedding(format!("nomic pooling failed: {e}")))?;
        let normalized = l2_normalize(&pooled)
            .map_err(|e| GlossError::Embedding(format!("nomic normalize failed: {e}")))?;
        let data: Vec<f32> = normalized
            .flatten_all()
            .and_then(|t| t.to_vec1())
            .map_err(|e| GlossError::Embedding(format!("nomic flatten failed: {e}")))?;
        Ok(data.chunks_exact(self.dims).map(|c| c.to_vec()).collect())
    }
}

/// Wrapper around an embedding backend. All heavy inference happens via candle
/// (in-process, CPU) or the crash-isolated Ollama process.
pub struct EmbeddingService {
    backend: EmbeddingBackend,
    dims: usize,
    model_id: String,
}

impl EmbeddingService {
    pub fn new_with_download_policy(
        cache_dir: &Path,
        use_gpu: bool,
        download_consent: bool,
    ) -> Result<Self, GlossError> {
        Self::new_with_download_policy_at(&hf_hub_cache_dir(), cache_dir, use_gpu, download_consent)
    }

    fn new_with_download_policy_at(
        hf_home: &Path,
        cache_dir: &Path,
        _use_gpu: bool,
        download_consent: bool,
    ) -> Result<Self, GlossError> {
        let repo_dir = hf_cache_repo_dir(hf_home);
        let cached = candle_model_is_cached(hf_home, cache_dir);

        let (config_path, weights_path, tokenizer_path) = if cached {
            // Make the snapshot resolvable even when refs/main was left empty
            // by an interrupted download or a partial cache tool.
            let revision = resolve_snapshot_revision(&repo_dir).ok_or_else(|| {
                GlossError::Embedding("cached nomic model snapshot is not resolvable".into())
            })?;
            let snapshot = repo_dir.join("snapshots").join(&revision);
            (
                snapshot.join("config.json"),
                snapshot.join("model.safetensors"),
                snapshot.join("tokenizer.json"),
            )
        } else {
            if !download_consent {
                return Err(GlossError::Embedding(
                    "The local embedding model (nomic-ai/nomic-embed-text-v1.5, ~550MB) is not \
                     downloaded yet. Enable “Automatically download the embedding model on first \
                     use” in Settings → Embeddings, or configure Ollama as the embedding backend."
                        .into(),
                ));
            }
            // First-time download via hf-hub (cached after this run).
            tracing::info!("Downloading nomic embedding model (first use)");
            let api = hf_hub::api::sync::ApiBuilder::new()
                .build()
                .map_err(|e| GlossError::Embedding(format!("hf-hub client build failed: {e}")))?;
            let repo = api.model(CANDLE_EMBEDDING_MODEL.to_string());
            let cfg = repo.get("config.json").map_err(|e| {
                GlossError::Embedding(format!("failed to download nomic config: {e}"))
            })?;
            let weights = repo.get("model.safetensors").map_err(|e| {
                GlossError::Embedding(format!("failed to download nomic weights: {e}"))
            })?;
            let tok = repo.get("tokenizer.json").map_err(|e| {
                GlossError::Embedding(format!("failed to download nomic tokenizer: {e}"))
            })?;
            (cfg, weights, tok)
        };

        let model = NomicV15Embedder::load_from_paths(
            &config_path,
            &weights_path,
            &tokenizer_path,
            NomicV15Embedder::MAX_LENGTH,
        )?;

        Ok(Self {
            backend: EmbeddingBackend::NomicV15(model),
            dims: 768,
            model_id: CANDLE_EMBEDDING_MODEL.to_string(),
        })
    }

    /// Build the embedder from app settings with an automatic CPU-candle
    /// fallback:
    ///
    /// - `provider == "ollama"` and reachable → Ollama `/api/embed`
    /// - otherwise (unset, "fastembed", "native", or an unreachable Ollama) →
    ///   the in-process CPU candle embedder (Nomic v1.5 MoE, 768d),
    ///   downloading the model on first use when consent is enabled.
    pub fn from_configured_provider(
        provider: Option<&str>,
        url: Option<&str>,
        model: Option<&str>,
        timeout_secs: Option<u64>,
        cache_dir: &Path,
        download_consent: bool,
    ) -> Result<Self, GlossError> {
        let wants_ollama = provider
            .map(|p| p.trim().eq_ignore_ascii_case("ollama"))
            .unwrap_or(false);

        if wants_ollama {
            let base_url = url
                .unwrap_or("http://localhost:11434")
                .trim()
                .trim_end_matches('/');
            let ollama_model = model.unwrap_or("bge-m3").trim();
            match Self::new_ollama(base_url, ollama_model, timeout_secs.unwrap_or(60)) {
                Ok(service) => {
                    tracing::info!(
                        url = %base_url,
                        model = %ollama_model,
                        "Ollama embedding backend ready"
                    );
                    return Ok(service);
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Ollama embedding backend unavailable; falling back to local CPU candle"
                    );
                }
            }
        } else {
            tracing::info!(
                provider = %provider.unwrap_or("auto"),
                "Using local CPU candle embedding backend"
            );
        }

        Self::new_with_download_policy(cache_dir, false, download_consent)
    }

    /// Ollama path — crash-isolated, preferred when explicitly configured.
    /// Probes the real embedding dimension at init so the HNSW index matches
    /// whatever model Ollama serves (e.g. bge-m3 → 1024 dims).
    ///
    /// Uses the ASYNC reqwest client: the blocking client panics with "Cannot
    /// drop a runtime in a context where blocking is not allowed" when created
    /// or dropped inside a tokio async context, which is exactly how
    /// `ensure_embedder` is called (warmup spawn, chat, import). That panic
    /// poisoned AppState locks and cascaded into "poisoned lock: another task
    /// failed inside" on every subsequent import — fixed by never touching
    /// reqwest::blocking here.
    pub fn new_ollama(url: &str, model: &str, timeout_secs: u64) -> Result<Self, GlossError> {
        let timeout = std::time::Duration::from_secs(timeout_secs.clamp(2, 300));
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| GlossError::Embedding(format!("HTTP client build failed: {e}")))?;

        let service = Self {
            backend: EmbeddingBackend::Ollama {
                client,
                url: url.trim_end_matches('/').to_string(),
                model: model.to_string(),
            },
            dims: 0,
            model_id: format!("ollama:{model}"),
        };
        let dims = service.probe_dimension()?;
        Ok(Self { dims, ..service })
    }

    fn probe_dimension(&self) -> Result<usize, GlossError> {
        let probe = self.embed_batch(&["gloss"])?;
        probe
            .first()
            .map(|v| v.len())
            .ok_or_else(|| GlossError::Embedding("Ollama probe embed returned no vectors".into()))
    }

    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, GlossError> {
        match &self.backend {
            EmbeddingBackend::NomicV15(model) => model.embed(texts),
            EmbeddingBackend::Ollama { client, url, model } => {
                ollama_embed_sync(client, url, model, texts)
            }
        }
    }

    pub fn dims(&self) -> usize {
        self.dims
    }

    pub fn provider_id(&self) -> &'static str {
        match &self.backend {
            EmbeddingBackend::Ollama { .. } => "ollama",
            EmbeddingBackend::NomicV15(_) => "fastembed-nomic-v2-moe",
        }
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    pub fn model_digest(&self) -> Option<String> {
        let mut hasher = Sha256::new();
        hasher.update(self.provider_id().as_bytes());
        hasher.update(b"\0");
        hasher.update(self.model_id.as_bytes());
        hasher.update(b"\0");
        hasher.update(self.dims.to_string().as_bytes());
        Some(format!("{:x}", hasher.finalize()))
    }

    /// Convenience: embed a single text by wrapping it in a batch of 1.
    pub fn embed_one(&self, text: &str) -> Result<Vec<f32>, GlossError> {
        let mut batch = self.embed_batch(&[text])?;
        batch
            .pop()
            .ok_or_else(|| GlossError::Embedding("embed_one returned empty batch".into()))
    }

    /// Rerank not supported with the candle or Ollama backends.
    pub fn has_reranker(&self) -> bool {
        false
    }

    /// Rerank not supported with the candle or Ollama backends.
    pub fn rerank(
        &self,
        _query: &str,
        _documents: &[String],
        _top_k: usize,
    ) -> Result<Vec<(usize, f32)>, GlossError> {
        Err(GlossError::Embedding(
            "current embedding backend does not support cross-encoder reranking".into(),
        ))
    }
}

/// Run an Ollama `/api/embed` request on the async reqwest client from both
/// sync and async callers without panicking. The old `reqwest::blocking`
/// client panicked ("Cannot drop a runtime in a context where blocking is not
/// allowed") whenever it was used or dropped inside a tokio async context —
/// which is how `ensure_embedder` and the import path call it. This mirrors
/// the semantic-memory adapter's proven `block_on_probe` pattern:
/// `block_in_place` inside an existing runtime, a throwaway current-thread
/// runtime otherwise.
fn ollama_embed_sync(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    texts: &[&str],
) -> Result<Vec<Vec<f32>>, GlossError> {
    let future = ollama_embed_request(client, url, model, texts);
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(future)),
        Err(_) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| {
                    GlossError::Embedding(format!("Ollama embed runtime build failed: {e}"))
                })?;
            runtime.block_on(future)
        }
    }
}

async fn ollama_embed_request(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    texts: &[&str],
) -> Result<Vec<Vec<f32>>, GlossError> {
    let body = serde_json::json!({
        "model": model,
        "input": texts,
    });
    let response = client
        .post(format!("{url}/api/embed"))
        .json(&body)
        .send()
        .await
        .map_err(|e| GlossError::Embedding(format!("Ollama embed request failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body_text = response
            .text()
            .await
            .unwrap_or_else(|_| "<could not read body>".to_string());
        return Err(GlossError::Embedding(format!(
            "Ollama embed returned HTTP {status}: {body_text}"
        )));
    }

    let parsed: serde_json::Value = response
        .json()
        .await
        .map_err(|e| GlossError::Embedding(format!("Ollama embed JSON parse failed: {e}")))?;

    let embeddings_array = parsed
        .get("embeddings")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            GlossError::Embedding("Ollama embed response missing \"embeddings\" array".into())
        })?;

    let mut out = Vec::with_capacity(embeddings_array.len());
    for emb in embeddings_array {
        let vector = emb
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                    .collect::<Vec<f32>>()
            })
            .ok_or_else(|| {
                GlossError::Embedding("Ollama embed entry is not a numeric array".into())
            })?;
        out.push(vector);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
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

        // Known pre-existing usearch FFI crash in teardown path; this test focuses
        // on add/search contract. We intentionally leak index here to keep CI green
        // without triggering static teardown on this path.
        std::mem::forget(index);
    }

    #[test]
    fn hnsw_index_grows_beyond_its_initial_capacity() {
        let mut index = HnswIndex::new(3).unwrap();

        for label in 0..=1_024 {
            assert_eq!(index.add(&[label as f32, 1.0, 0.0]).unwrap(), label);
        }

        assert_eq!(index.size(), 1_025);

        // See hnsw_index_create_and_search for the usearch teardown workaround.
        std::mem::forget(index);
    }

    #[test]
    fn verified_save_publishes_the_canonical_artifact_and_reloads_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = native_dense_artifact_path(dir.path());
        let mut index = HnswIndex::new(3).unwrap();
        index.add(&[1.0, 0.0, 0.0]).unwrap();

        index.save_atomic_verified(&path, 1).unwrap();
        assert!(path.exists());
        let reloaded = HnswIndex::load_with_hwm(&path, 1, 3).unwrap();
        assert_eq!(reloaded.search(&[1.0, 0.0, 0.0], 1).unwrap()[0].0, 0);

        std::mem::forget(reloaded);
        std::mem::forget(index);
    }

    #[test]
    fn candle_model_cache_detection_and_ref_repair() {
        let dir = tempfile::tempdir().unwrap();
        let hf_home = dir.path().join("hf");
        let repo = hf_cache_repo_dir(&hf_home);
        let snapshot = repo.join("snapshots").join("deadbeef");
        std::fs::create_dir_all(&snapshot).unwrap();
        std::fs::write(snapshot.join("config.json"), "{}").unwrap();
        std::fs::write(snapshot.join("model.safetensors"), "fake").unwrap();
        std::fs::write(snapshot.join("tokenizer.json"), "{}").unwrap();

        // Files present in a snapshot revision → cached (no consent needed).
        assert!(candle_model_is_cached(&hf_home, Path::new("/nonexistent")));

        // Empty refs/main gets repaired when exactly one revision exists.
        std::fs::create_dir_all(repo.join("refs")).unwrap();
        std::fs::write(repo.join("refs").join("main"), "").unwrap();
        repair_hf_refs_if_needed(&repo);
        assert_eq!(
            std::fs::read_to_string(repo.join("refs").join("main")).unwrap(),
            "deadbeef"
        );

        // Missing model files → not cached.
        let empty = tempfile::tempdir().unwrap();
        assert!(!candle_model_is_cached(
            empty.path(),
            Path::new("/nonexistent")
        ));

        // Sharded weights also count as cached.
        let sharded = tempfile::tempdir().unwrap();
        let shard_repo = hf_cache_repo_dir(sharded.path());
        let shard_snapshot = shard_repo.join("snapshots").join("abc123");
        std::fs::create_dir_all(&shard_snapshot).unwrap();
        std::fs::write(shard_snapshot.join("config.json"), "{}").unwrap();
        std::fs::write(
            shard_snapshot.join("model-00001-of-00002.safetensors"),
            "fake",
        )
        .unwrap();
        std::fs::write(shard_snapshot.join("tokenizer.json"), "{}").unwrap();
        assert!(candle_model_is_cached(
            sharded.path(),
            Path::new("/nonexistent")
        ));
    }

    #[test]
    fn consent_required_only_when_model_is_missing() {
        let empty = tempfile::tempdir().unwrap();
        let err =
            EmbeddingService::new_with_download_policy_at(empty.path(), empty.path(), false, false)
                .err()
                .expect("missing model without consent must fail");
        let msg = format!("{err}");
        assert!(msg.contains("not downloaded"), "unexpected error: {msg}");
    }

    #[test]
    fn ollama_backend_probes_dimension_and_embeds() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            // Serve two requests: the init probe and the embed_one call.
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 8192];
                let _ = stream.read(&mut buf).unwrap();
                let body = r#"{"embeddings":[[0.1,0.2,0.3]]}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let service = EmbeddingService::new_ollama(&format!("http://{addr}"), "bge-m3", 5).unwrap();
        assert_eq!(service.dims(), 3);
        assert_eq!(service.provider_id(), "ollama");

        let emb = service.embed_one("hello").unwrap();
        assert_eq!(emb.len(), 3);
        assert_eq!(emb[1], 0.2);

        handle.join().unwrap();
    }

    /// Manual smoke test: with the real nomic model present in the default
    /// hf-hub cache, `new_with_download_policy` must succeed WITHOUT download
    /// consent and produce a 768-dim embedding. Run with:
    /// `cargo test --features semantic-memory-turbo-quant ingestion::embed::tests::real_cache_loads_without_consent -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_cache_loads_without_consent() {
        let cache_dir = std::env::temp_dir().join("gloss-embed-smoke-cache");
        let service = EmbeddingService::new_with_download_policy(&cache_dir, false, false)
            .expect("model cached → no consent required");
        assert_eq!(service.dims(), 768);
        assert_eq!(service.provider_id(), "fastembed-nomic-v2-moe");

        let emb = service.embed_one("Gloss embedding smoke test").unwrap();
        assert_eq!(emb.len(), 768);
        let norm: f32 = emb.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!(
            norm > 0.9 && norm < 1.1,
            "expected unit vector, norm={norm}"
        );
        println!("smoke ok: 768-dim unit vector from cached candle model");
    }
}
