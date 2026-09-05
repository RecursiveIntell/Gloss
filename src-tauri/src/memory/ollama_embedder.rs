//! Mechanical registry Embedder bridge to Gloss's canonical Ollama transport.
//! The registry still owns memory writes, retrieval and projection semantics.
use crate::error::GlossError;
use crate::ingestion::embedding_contract::{ollama_client, ollama_embed_request};
use semantic_memory::embedder::{EmbedBatchFuture, EmbedFuture};
use semantic_memory::{Embedder, MemoryError};

pub struct GlossOllamaEmbedder {
    client: reqwest::Client,
    url: String,
    model: String,
    dimensions: usize,
}

impl GlossOllamaEmbedder {
    pub fn try_new(
        url: &str,
        model: &str,
        dimensions: usize,
        timeout_secs: u64,
        allow_lan: bool,
    ) -> Result<Self, GlossError> {
        if dimensions == 0 {
            return Err(GlossError::Embedding(
                "Embedding dimensions must be positive".into(),
            ));
        }
        Ok(Self {
            client: ollama_client(url, model, timeout_secs, allow_lan)?,
            url: url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            dimensions,
        })
    }
}

impl Embedder for GlossOllamaEmbedder {
    fn embed<'a>(&'a self, text: &'a str) -> EmbedFuture<'a> {
        Box::pin(async move {
            let mut vectors = self.embed_batch(vec![text.to_string()]).await?;
            vectors
                .pop()
                .ok_or_else(|| MemoryError::Other("Ollama returned no vector".into()))
        })
    }

    fn embed_batch<'a>(&'a self, texts: Vec<String>) -> EmbedBatchFuture<'a> {
        Box::pin(async move {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
            // Direct await is essential: registry timeout/cancellation drops
            // this HTTP future before the caller's inference guard is released.
            let vectors = ollama_embed_request(&self.client, &self.url, &self.model, &texts)
                .await
                .map_err(|error| MemoryError::EmbedderUnavailable(error.to_string()))?;
            for vector in &vectors {
                if vector.len() != self.dimensions {
                    return Err(MemoryError::DimensionMismatch {
                        expected: self.dimensions,
                        actual: vector.len(),
                    });
                }
            }
            Ok(vectors)
        })
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}
