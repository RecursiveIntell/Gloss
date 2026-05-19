use semantic_memory::embedder::{EmbedBatchFuture, EmbedFuture};
use semantic_memory::Embedder;
use semantic_memory::{MemoryConfig, MemoryStore};
use tempfile::TempDir;

#[derive(Clone)]
enum BatchMode {
    Fewer,
    More,
    NonFinite,
}

struct BadBatchEmbedder {
    dims: usize,
    mode: BatchMode,
}

impl BadBatchEmbedder {
    fn new(dims: usize, mode: BatchMode) -> Self {
        Self { dims, mode }
    }

    fn valid_embedding(&self) -> Vec<f32> {
        let mut values = vec![0.0; self.dims];
        if let Some(first) = values.first_mut() {
            *first = 1.0;
        }
        values
    }
}

impl Embedder for BadBatchEmbedder {
    fn embed<'a>(&'a self, _text: &'a str) -> EmbedFuture<'a> {
        let embedding = self.valid_embedding();
        Box::pin(async move { Ok(embedding) })
    }

    fn embed_batch<'a>(&'a self, texts: Vec<String>) -> EmbedBatchFuture<'a> {
        let dims = self.dims;
        let mode = self.mode.clone();
        Box::pin(async move {
            let mut embeddings = vec![vec![1.0; dims]; texts.len()];
            match mode {
                BatchMode::Fewer => {
                    embeddings.pop();
                }
                BatchMode::More => embeddings.push(vec![1.0; dims]),
                BatchMode::NonFinite => {
                    if let Some(first) = embeddings.first_mut().and_then(|v| v.first_mut()) {
                        *first = f32::NAN;
                    }
                }
            }
            Ok(embeddings)
        })
    }

    fn model_name(&self) -> &str {
        "bad-batch"
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

fn store_with(embedder: BadBatchEmbedder) -> (MemoryStore, TempDir) {
    let tmp = TempDir::new().unwrap();
    let config = MemoryConfig {
        base_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let store = MemoryStore::open_with_embedder(config, Box::new(embedder)).unwrap();
    (store, tmp)
}

#[tokio::test]
async fn document_ingest_rejects_short_embedding_batch_before_write() {
    let (store, _tmp) = store_with(BadBatchEmbedder::new(768, BatchMode::Fewer));
    let err = store
        .ingest_document(
            "doc",
            "first paragraph\n\nsecond paragraph",
            "ns",
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "embedding_batch_count_mismatch");
}

#[tokio::test]
async fn document_ingest_rejects_long_embedding_batch_before_write() {
    let (store, _tmp) = store_with(BadBatchEmbedder::new(768, BatchMode::More));
    let err = store
        .ingest_document(
            "doc",
            "first paragraph\n\nsecond paragraph",
            "ns",
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "embedding_batch_count_mismatch");
}

#[tokio::test]
async fn document_ingest_rejects_non_finite_embedding_before_write() {
    let (store, _tmp) = store_with(BadBatchEmbedder::new(768, BatchMode::NonFinite));
    let err = store
        .ingest_document(
            "doc",
            "first paragraph\n\nsecond paragraph",
            "ns",
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "non_finite_embedding_value");
}

#[tokio::test]
async fn public_embedding_write_rejects_wrong_dimension() {
    let (store, _tmp) = store_with(BadBatchEmbedder::new(768, BatchMode::More));
    let err = store
        .add_fact_with_embedding("ns", "content", &[1.0, 2.0], None, None)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "embedding_dimension_mismatch");
}

#[tokio::test]
async fn public_embedding_write_rejects_non_finite_value() {
    let (store, _tmp) = store_with(BadBatchEmbedder::new(768, BatchMode::More));
    let mut embedding = vec![0.0; 768];
    embedding[3] = f32::NEG_INFINITY;
    let err = store
        .add_fact_with_embedding("ns", "content", &embedding, None, None)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "non_finite_embedding_value");
}
