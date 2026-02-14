use crate::error::GlossError;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use std::path::Path;
use usearch::ffi::{IndexOptions, MetricKind, ScalarKind};

/// Wrapper around fastembed TextEmbedding for generating embeddings.
pub struct EmbeddingService {
    model: TextEmbedding,
}

impl EmbeddingService {
    /// Initialize the embedding service with NomicEmbedTextV15 (768-dim).
    pub fn new(cache_dir: &Path) -> Result<Self, GlossError> {
        let options = InitOptions::new(EmbeddingModel::NomicEmbedTextV15)
            .with_cache_dir(cache_dir.to_path_buf());

        let model = TextEmbedding::try_new(options).map_err(|e| {
            GlossError::Embedding(format!("Failed to initialize embedding model: {}", e))
        })?;

        Ok(Self { model })
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

    /// Embed a batch of texts.
    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, GlossError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let owned: Vec<String> = texts.iter().map(|t| t.to_string()).collect();
        self.model
            .embed(owned, None)
            .map_err(|e| GlossError::Embedding(format!("Batch embedding failed: {}", e)))
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
    pub fn add(&mut self, vector: &[f32]) -> Result<u64, GlossError> {
        let label = self.next_label;
        self.index.add(label, vector).map_err(|e| {
            GlossError::Embedding(format!("Failed to add vector to index: {}", e))
        })?;
        self.next_label += 1;
        Ok(label)
    }

    /// Search for the K nearest neighbors.
    pub fn search(&self, query: &[f32], k: usize) -> Result<Vec<(u64, f32)>, GlossError> {
        if self.index.size() == 0 {
            return Ok(Vec::new());
        }
        let results = self.index.search(query, k).map_err(|e| {
            GlossError::Embedding(format!("HNSW search failed: {}", e))
        })?;
        Ok(results
            .keys
            .into_iter()
            .zip(results.distances)
            .collect())
    }

    /// Save the index to disk.
    pub fn save(&self, path: &Path) -> Result<(), GlossError> {
        self.index.save(path.to_str().unwrap_or("")).map_err(|e| {
            GlossError::Embedding(format!("Failed to save HNSW index: {}", e))
        })
    }

    /// Load an index from disk.
    pub fn load(path: &Path) -> Result<Self, GlossError> {
        let options = IndexOptions {
            dimensions: 768,
            metric: MetricKind::Cos,
            quantization: ScalarKind::F32,
            ..Default::default()
        };
        let index = usearch::new_index(&options)
            .map_err(|e| GlossError::Embedding(format!("Failed to create index for load: {}", e)))?;
        index
            .load(path.to_str().unwrap_or(""))
            .map_err(|e| GlossError::Embedding(format!("Failed to load HNSW index: {}", e)))?;
        let next_label = index.size() as u64;
        Ok(Self { index, next_label })
    }

    /// Get the number of vectors in the index.
    pub fn size(&self) -> usize {
        self.index.size()
    }
}
