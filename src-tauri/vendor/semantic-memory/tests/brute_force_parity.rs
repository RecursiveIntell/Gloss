//! Brute-force parity tests: verify both backends produce consistent results.
//!
//! Only compiled when BOTH `hnsw` and `brute-force` features are enabled.
//! Since the backend is chosen at compile time (not runtime), this test
//! verifies that the public search API works correctly when both features
//! are enabled simultaneously.

#![cfg(all(feature = "hnsw", feature = "brute-force"))]

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

#[tokio::test]
async fn both_features_enabled_search_works() {
    let (store, _tmp) = test_store();

    // Populate with facts
    for i in 0..20 {
        store
            .add_fact(
                "parity",
                &format!("Parity test fact number {} about various topics", i),
                None,
                None,
            )
            .await
            .unwrap();
    }

    // Hybrid search (uses HNSW when hnsw feature is default)
    let hybrid_results = store
        .search("parity test fact", Some(5), None, None)
        .await
        .unwrap();

    // FTS-only search works regardless of backend
    let fts_results = store
        .search_fts_only("parity test fact", Some(5), None, None)
        .await
        .unwrap();

    // Vector-only search
    let _vector_results = store
        .search_vector_only("parity test", Some(5), None, None)
        .await
        .unwrap();

    // All should return results
    assert!(
        !hybrid_results.is_empty(),
        "Hybrid search should find results with both features"
    );
    assert!(
        !fts_results.is_empty(),
        "FTS search should find results with both features"
    );
    // Vector-only with MockEmbedder may not find much, but shouldn't error
    // (MockEmbedder generates hash-based embeddings, not truly semantic)

    // FTS results should overlap with hybrid results
    let hybrid_contents: Vec<&str> = hybrid_results.iter().map(|r| r.content.as_str()).collect();
    let fts_contents: Vec<&str> = fts_results.iter().map(|r| r.content.as_str()).collect();

    let overlap = hybrid_contents
        .iter()
        .filter(|c| fts_contents.contains(c))
        .count();
    assert!(
        overlap > 0,
        "Hybrid and FTS results should have at least some overlap"
    );
}

#[tokio::test]
async fn both_features_handle_empty_store() {
    let (store, _tmp) = test_store();

    let results = store
        .search("nonexistent", Some(5), None, None)
        .await
        .unwrap();
    assert!(results.is_empty(), "Empty store should return no results");

    let fts = store
        .search_fts_only("nonexistent", Some(5), None, None)
        .await
        .unwrap();
    assert!(
        fts.is_empty(),
        "FTS on empty store should return no results"
    );
}

#[tokio::test]
async fn both_features_handle_delete() {
    let (store, _tmp) = test_store();

    let id = store
        .add_fact("ns", "Deletable fact for parity testing", None, None)
        .await
        .unwrap();

    // Should be findable before delete
    let before = store
        .search_fts_only("Deletable parity", Some(5), None, None)
        .await
        .unwrap();
    assert!(!before.is_empty());

    store.delete_fact(&id).await.unwrap();

    // Should be gone from both paths
    let after_fts = store
        .search_fts_only("Deletable parity", Some(5), None, None)
        .await
        .unwrap();
    assert!(after_fts.is_empty(), "FTS should not find deleted fact");

    let after_hybrid = store
        .search("Deletable parity", Some(5), None, None)
        .await
        .unwrap();
    assert!(
        after_hybrid.is_empty(),
        "Hybrid search should not find deleted fact"
    );
}
