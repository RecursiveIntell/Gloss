use crate::error::GlossError;
use fastembed::{
    EmbeddingModel, InitOptions, RerankInitOptions, RerankerModel, TextEmbedding, TextRerank,
};
use std::path::Path;
use usearch::ffi::{IndexOptions, MetricKind, ScalarKind};

/// Wrapper around fastembed TextEmbedding for generating embeddings and reranking.
pub struct EmbeddingService {
    model: TextEmbedding,
    reranker: Option<TextRerank>,
}

impl EmbeddingService {
    /// Initialize the embedding service with NomicEmbedTextV15 (768-dim)
    /// and BGERerankerBase for cross-encoder reranking.
    pub fn new(cache_dir: &Path) -> Result<Self, GlossError> {
        let options = InitOptions::new(EmbeddingModel::NomicEmbedTextV15)
            .with_cache_dir(cache_dir.to_path_buf());

        let model = TextEmbedding::try_new(options).map_err(|e| {
            GlossError::Embedding(format!("Failed to initialize embedding model: {}", e))
        })?;

        // Non-fatal: fall back to RRF-only if reranker fails to load
        let reranker = match TextRerank::try_new(
            RerankInitOptions::new(RerankerModel::BGERerankerBase)
                .with_cache_dir(cache_dir.to_path_buf()),
        ) {
            Ok(r) => {
                tracing::info!("Reranker (BGERerankerBase) loaded");
                Some(r)
            }
            Err(e) => {
                tracing::warn!("Reranker failed to load (falling back to RRF-only): {}", e);
                None
            }
        };

        Ok(Self { model, reranker })
    }

    /// Embed a single text string.
    pub fn embed_one(&self, text: &str) -> Result<Vec<f32>, GlossError> {
        let results = self
            .model
            .embed(vec![text], None)
            .map_err(|e| GlossError::Embedding(format!("Embedding failed: {}", e)))?;
        results
            .into_iter()
            .next()
            .ok_or_else(|| GlossError::Embedding("No embedding produced".into()))
    }

    /// Embed a batch of texts. Uses adaptive sub-batch sizing based on average
    /// text length to limit peak GPU memory from ONNX runtime intermediate tensors.
    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, GlossError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // Adaptive batching: reduce batch size for longer texts to limit
        // peak GPU memory from ONNX runtime intermediate tensors.
        let avg_len = texts.iter().map(|t| t.len()).sum::<usize>() / texts.len().max(1);
        let sub_batch: usize = if avg_len > 4000 {
            8
        } else if avg_len > 2000 {
            16
        } else if avg_len > 1000 {
            32
        } else {
            48
        };

        let mut all = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(sub_batch) {
            let owned: Vec<String> = chunk.iter().map(|t| t.to_string()).collect();
            let batch = self
                .model
                .embed(owned, None)
                .map_err(|e| GlossError::Embedding(format!("Batch embedding failed: {}", e)))?;
            all.extend(batch);
        }
        Ok(all)
    }

    /// Rerank documents against a query using cross-encoder.
    /// Returns (original_index, relevance_score) sorted by score descending.
    /// If reranker is not loaded, returns passthrough indices 0..top_k.
    pub fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_k: usize,
    ) -> Result<Vec<(usize, f32)>, GlossError> {
        let reranker = match &self.reranker {
            Some(r) => r,
            None => {
                // Passthrough: return first top_k indices with dummy scores
                return Ok((0..top_k.min(documents.len()))
                    .map(|i| (i, 1.0 - (i as f32 * 0.01)))
                    .collect());
            }
        };

        let doc_refs: Vec<&str> = documents.iter().map(|s| s.as_str()).collect();
        let results = reranker
            .rerank(query, doc_refs, false, None)
            .map_err(|e| GlossError::Embedding(format!("Reranking failed: {}", e)))?;

        Ok(results
            .into_iter()
            .take(top_k)
            .map(|r| (r.index, r.score))
            .collect())
    }

    /// Whether the reranker is available.
    pub fn has_reranker(&self) -> bool {
        self.reranker.is_some()
    }
}

/// HNSW vector index wrapper around usearch.
pub struct HnswIndex {
    index: usearch::Index,
    next_label: u64,
}

impl HnswIndex {
    /// Create a new empty HNSW index for 768-dimensional vectors.
    pub fn new() -> Result<Self, GlossError> {
        let options = IndexOptions {
            dimensions: 768,
            metric: MetricKind::Cos,
            quantization: ScalarKind::F32,
            ..Default::default()
        };
        let index = usearch::new_index(&options)
            .map_err(|e| GlossError::Embedding(format!("Failed to create HNSW index: {}", e)))?;
        index
            .reserve(10000)
            .map_err(|e| GlossError::Embedding(format!("Failed to reserve index space: {}", e)))?;
        Ok(Self {
            index,
            next_label: 0,
        })
    }

    /// Add a vector to the index. Returns the label assigned.
    /// Automatically grows capacity when the index is full.
    pub fn add(&mut self, vector: &[f32]) -> Result<u64, GlossError> {
        // Grow capacity before the C++ layer overflows
        if self.index.size() >= self.index.capacity() {
            let new_cap = self.index.capacity() + 10_000;
            self.index
                .reserve(new_cap)
                .map_err(|e| GlossError::Embedding(format!("Failed to grow HNSW index: {}", e)))?;
        }
        let label = self.next_label;
        self.index
            .add(label, vector)
            .map_err(|e| GlossError::Embedding(format!("Failed to add vector to index: {}", e)))?;
        self.next_label += 1;
        Ok(label)
    }

    /// Search for the K nearest neighbors.
    pub fn search(&self, query: &[f32], k: usize) -> Result<Vec<(u64, f32)>, GlossError> {
        if self.index.size() == 0 {
            return Ok(Vec::new());
        }
        let results = self
            .index
            .search(query, k)
            .map_err(|e| GlossError::Embedding(format!("HNSW search failed: {}", e)))?;
        Ok(results.keys.into_iter().zip(results.distances).collect())
    }

    /// Save the index to disk.
    pub fn save(&self, path: &Path) -> Result<(), GlossError> {
        self.index
            .save(path.to_str().unwrap_or(""))
            .map_err(|e| GlossError::Embedding(format!("Failed to save HNSW index: {}", e)))
    }

    /// Load an index from disk (convenience — delegates to `load_with_hwm` with no DB hint).
    pub fn load(path: &Path) -> Result<Self, GlossError> {
        Self::load_with_hwm(path, None)
    }

    /// Load an index from disk, using the provided high-water mark for label safety.
    /// After deletions, `index.size()` returns current count, not max label ever assigned.
    /// The DB high-water mark prevents label reuse and embedding_id collisions.
    pub fn load_with_hwm(path: &Path, max_embedding_id: Option<i64>) -> Result<Self, GlossError> {
        let options = IndexOptions {
            dimensions: 768,
            metric: MetricKind::Cos,
            quantization: ScalarKind::F32,
            ..Default::default()
        };
        let index = usearch::new_index(&options).map_err(|e| {
            GlossError::Embedding(format!("Failed to create index for load: {}", e))
        })?;
        index
            .load(path.to_str().unwrap_or(""))
            .map_err(|e| GlossError::Embedding(format!("Failed to load HNSW index: {}", e)))?;
        let next_label = match max_embedding_id {
            Some(max_id) => (max_id as u64) + 1,
            None => index.size() as u64,
        };
        tracing::debug!(
            index_size = index.size(),
            max_embedding_id = ?max_embedding_id,
            next_label,
            "HNSW index loaded with high-water mark"
        );
        Ok(Self { index, next_label })
    }

    /// Remove a vector from the index by key. Best-effort — some index
    /// configurations may not support removal.
    pub fn remove(&mut self, key: u64) -> Result<(), GlossError> {
        self.index
            .remove(key)
            .map_err(|e| GlossError::Embedding(format!("HNSW remove failed: {}", e)))?;
        Ok(())
    }

    /// Get the number of vectors in the index.
    pub fn size(&self) -> usize {
        self.index.size()
    }
}
