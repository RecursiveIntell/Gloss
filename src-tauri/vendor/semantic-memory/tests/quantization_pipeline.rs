#![cfg(feature = "hnsw")]

use semantic_memory::{MemoryConfig, MemoryStore, MockEmbedder};
use tempfile::TempDir;

fn test_store() -> (MemoryStore, TempDir) {
    let tmp = TempDir::new().unwrap();
    let config = MemoryConfig {
        base_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let embedder = Box::new(MockEmbedder::new(768));
    let store = MemoryStore::open_with_embedder(config, embedder).unwrap();
    (store, tmp)
}

/// 5.1 - Adding a fact stores both f32 and q8 embeddings. Verify indirectly:
/// add a fact, close, reopen with a fresh MemoryStore, and search for it.
#[tokio::test]
async fn add_fact_stores_q8() {
    let tmp = TempDir::new().unwrap();

    // Open, add a fact, then drop the store.
    {
        let config = MemoryConfig {
            base_dir: tmp.path().to_path_buf(),
            ..Default::default()
        };
        let embedder = Box::new(MockEmbedder::new(768));
        let store = MemoryStore::open_with_embedder(config, embedder).unwrap();

        store
            .add_fact(
                "general",
                "quantization pipeline stores q8 embeddings correctly",
                None,
                None,
            )
            .await
            .unwrap();
    }

    // Reopen the store from the same directory.
    let config = MemoryConfig {
        base_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let embedder = Box::new(MockEmbedder::new(768));
    let store = MemoryStore::open_with_embedder(config, embedder).unwrap();

    // The fact should be findable (embeddings, including q8, were persisted).
    let results = store
        .search("quantization pipeline", Some(5), None, None)
        .await
        .unwrap();
    assert!(
        !results.is_empty(),
        "Fact should be found after close/reopen, confirming embeddings were stored"
    );
}

/// 5.2 - reembed_all regenerates embeddings (including q8) and search still works.
#[tokio::test]
async fn reembed_all_regenerates_q8() {
    let (store, _tmp) = test_store();

    // Add several facts.
    for i in 0..5 {
        store
            .add_fact(
                "general",
                &format!("reembed fact number {} with quantized vectors", i),
                None,
                None,
            )
            .await
            .unwrap();
    }

    // Re-embed everything (regenerates both f32 and q8 embeddings).
    let count = store.reembed_all().await.unwrap();
    assert_eq!(count, 5, "Should re-embed all 5 facts");

    // Search should still work after re-embedding.
    let results = store
        .search("reembed quantized vectors", Some(5), None, None)
        .await
        .unwrap();
    assert!(
        !results.is_empty(),
        "Search should return results after reembed_all"
    );
}
