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

/// 3.1 - Rebuild the HNSW index and verify it is a live swap (no reopen required).
#[tokio::test]
async fn rebuild_updates_live_instance() {
    let (store, _tmp) = test_store();

    // Add 10 facts with distinct content so FTS can find them.
    let mut fact_ids = Vec::new();
    for i in 0..10 {
        let id = store
            .add_fact(
                "general",
                &format!("fact number {} about rebuilding hnsw indexes", i),
                None,
                None,
            )
            .await
            .unwrap();
        fact_ids.push(id);
    }

    // Rebuild the HNSW index (hot-swap via RwLock).
    store.rebuild_hnsw_index().await.unwrap();

    // Search should still find the facts without reopening the store.
    let results = store
        .search("rebuilding hnsw indexes", Some(10), None, None)
        .await
        .unwrap();
    assert!(
        !results.is_empty(),
        "Search should return results after HNSW rebuild"
    );
}

/// 3.2 - Concurrent search during rebuild must not panic.
#[tokio::test]
async fn concurrent_search_during_rebuild() {
    let (store, _tmp) = test_store();

    // Seed 20 facts.
    for i in 0..20 {
        store
            .add_fact(
                "general",
                &format!("concurrent fact number {} for stress test search", i),
                None,
                None,
            )
            .await
            .unwrap();
    }

    let search_store = store.clone();
    let rebuild_store = store.clone();

    let search_handle = tokio::spawn(async move {
        for _ in 0..10 {
            let _ = search_store
                .search("stress test search", Some(5), None, None)
                .await;
        }
        Ok::<(), semantic_memory::MemoryError>(())
    });

    let rebuild_handle = tokio::spawn(async move { rebuild_store.rebuild_hnsw_index().await });

    // Both tasks must complete without panic.
    let (search_result, rebuild_result) = tokio::try_join!(search_handle, rebuild_handle).unwrap();
    search_result.unwrap();
    rebuild_result.unwrap();
}
