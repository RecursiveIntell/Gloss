//! Concurrent access tests: multi-task safety for MemoryStore.
//!
//! Verifies that concurrent inserts and searches don't panic or deadlock.

use semantic_memory::{MemoryConfig, MemoryStore, MockEmbedder};
use std::sync::Arc;
use std::time::Duration;
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
async fn concurrent_insert_and_search_no_panic() {
    let (store, _tmp) = test_store();
    let store = Arc::new(store);

    // Spawn inserter
    let insert_store = store.clone();
    let inserter = tokio::spawn(async move {
        for i in 0..50 {
            insert_store
                .add_fact(
                    "concurrent",
                    &format!("Concurrent fact number {}", i),
                    None,
                    None,
                )
                .await
                .unwrap();
        }
    });

    // Spawn searcher concurrently
    let search_store = store.clone();
    let searcher = tokio::spawn(async move {
        for _ in 0..20 {
            // Search may return 0 or more results depending on timing
            let _ = search_store.search("concurrent", Some(5), None, None).await;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });

    // Both should complete within 30 seconds (deadlock detection)
    let timeout = Duration::from_secs(30);
    let result = tokio::time::timeout(timeout, async {
        let (r1, r2) = tokio::join!(inserter, searcher);
        r1.unwrap();
        r2.unwrap();
    })
    .await;

    assert!(
        result.is_ok(),
        "Concurrent insert + search should complete without deadlock"
    );
}

#[tokio::test]
async fn concurrent_multiple_inserters() {
    let (store, _tmp) = test_store();
    let store = Arc::new(store);

    let mut handles = Vec::new();

    // Spawn 5 inserters
    for task_id in 0..5 {
        let s = store.clone();
        handles.push(tokio::spawn(async move {
            for i in 0..10 {
                s.add_fact("multi", &format!("Task {} fact {}", task_id, i), None, None)
                    .await
                    .unwrap();
            }
        }));
    }

    let timeout = Duration::from_secs(30);
    let result = tokio::time::timeout(timeout, async {
        for h in handles {
            h.await.unwrap();
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "Multiple concurrent inserters should complete without deadlock"
    );

    // All 50 facts should be present
    let stats = store.stats().await.unwrap();
    assert_eq!(
        stats.total_facts, 50,
        "All concurrent inserts should succeed"
    );
}

#[tokio::test]
async fn clone_store_shares_state_across_tasks() {
    let (store, _tmp) = test_store();

    let store_a = store.clone();
    let store_b = store.clone();

    // Insert via clone A
    store_a
        .add_fact("shared", "Shared state verification", None, None)
        .await
        .unwrap();

    // Search via clone B
    let results = store_b
        .search_fts_only("Shared state", Some(5), None, None)
        .await
        .unwrap();
    assert!(
        !results.is_empty(),
        "Cloned stores should share the same underlying state"
    );
}
