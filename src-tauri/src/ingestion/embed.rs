use crate::error::GlossError;
use fastembed::NomicV2MoeTextEmbedding;
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

// Note: NomicV2MoeTextEmbedding uses candle-core internally via fastembed's nomic-v2-moe feature

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

/// Embedding backend: FastEmbed only (Nomic v2 MoE via candle, no ONNX/ort).
/// This unblocks TTS (any-tts also uses candle-core).
pub enum EmbeddingBackend {
    NomicV2Moe(NomicV2MoeTextEmbedding),
}

/// Wrapper around an embedding backend. All heavy inference happens via candle.
/// Uses nomic-ai/nomic-embed-text-v1.5 (768 dimensions) by default.
pub struct EmbeddingService {
    backend: EmbeddingBackend,
    #[allow(dead_code)]
    dims: usize,
    model_id: String,
}

impl EmbeddingService {
    pub fn new_with_download_policy(
        _cache_dir: &Path,
        _use_gpu: bool,
        download_consent: bool,
    ) -> Result<Self, GlossError> {
        // For candle backend, we don't use the same cache mechanism.
        // The model downloads to ~/.cache/huggingface/hub automatically.
        // We accept download consent as a signal that HF hub is accessible.
        if !download_consent {
            // Check if HF cache already has the model
            let hf_cache = directories::ProjectDirs::from("com", "gloss", "gloss")
                .map(|p| p.cache_dir().to_path_buf())
                .unwrap_or_else(|| std::env::temp_dir().to_path_buf())
                .join("huggingface")
                .join("hub");
            let cache_path = hf_cache.join("models--nomic-ai--nomic-embed-text-v1.5");
            if !cache_path.exists() {
                return Err(GlossError::Embedding(
                    "FastEmbed download consent required for first-time model download".into(),
                ));
            }
        }

        let device = candle_core::Device::Cpu;
        let model = NomicV2MoeTextEmbedding::from_hf(
            "nomic-ai/nomic-embed-text-v1.5",
            &device,
            candle_core::DType::F32,
            2048, // max_length
        )
        .map_err(|e| GlossError::Embedding(format!("NomicV2Moe init failed: {e}")))?;

        Ok(Self {
            backend: EmbeddingBackend::NomicV2Moe(model),
            dims: 768,
            model_id: "nomic-ai/nomic-embed-text-v1.5".to_string(),
        })
    }

    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, GlossError> {
        match &self.backend {
            EmbeddingBackend::NomicV2Moe(model) => {
                let embeddings = model
                    .embed(texts)
                    .map_err(|e| GlossError::Embedding(format!("Nomic embed failed: {e}")))?;
                Ok(embeddings)
            }
        }
    }

    #[allow(dead_code)]
    pub fn dims(&self) -> usize {
        self.dims
    }

    pub fn provider_id(&self) -> &'static str {
        "fastembed-nomic-v2-moe"
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    pub fn model_digest(&self) -> Option<String> {
        let mut hasher = Sha256::new();
        hasher.update(b"fastembed-nomic-v2-moe");
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

    /// Rerank not supported with candle-based Nomic v2 MoE.
    pub fn has_reranker(&self) -> bool {
        false
    }

    /// Rerank not supported with candle-based Nomic v2 MoE.
    pub fn rerank(
        &self,
        _query: &str,
        _documents: &[String],
        _top_k: usize,
    ) -> Result<Vec<(usize, f32)>, GlossError> {
        Err(GlossError::Embedding(
            "Nomic v2 MoE backend does not support cross-encoder reranking".into(),
        ))
    }
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
}
