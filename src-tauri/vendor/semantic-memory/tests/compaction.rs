#![cfg(feature = "hnsw")]

use semantic_memory::{MemoryConfig, MemoryStore, MockEmbedder, SearchSourceType};
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

/// 7.1 - Add 10 facts, delete 3, verify only 7 remain via search/stats.
#[tokio::test]
async fn deleted_ratio_computation() {
    let (store, _tmp) = test_store();

    let mut fact_ids = Vec::new();
    for i in 0..10 {
        let id = store
            .add_fact(
                "general",
                &format!("deletable fact number {} for ratio test", i),
                None,
                None,
            )
            .await
            .unwrap();
        fact_ids.push(id);
    }

    // Delete the first 3 facts.
    for id in &fact_ids[..3] {
        store.delete_fact(id).await.unwrap();
    }

    // Stats should show 7 remaining.
    let stats = store.stats().await.unwrap();
    assert_eq!(
        stats.total_facts, 7,
        "Should have 7 facts remaining after deleting 3"
    );

    // Verify the deleted facts are gone.
    for id in &fact_ids[..3] {
        let fact = store.get_fact(id).await.unwrap();
        assert!(fact.is_none(), "Deleted fact {} should not be found", id);
    }

    // Verify the remaining facts still exist.
    for id in &fact_ids[3..] {
        let fact = store.get_fact(id).await.unwrap();
        assert!(fact.is_some(), "Remaining fact {} should still exist", id);
    }
}

/// 7.2 - Add 20 facts, delete 10, compact, verify search returns correct results.
#[tokio::test]
async fn compact_reduces_tombstones() {
    let (store, _tmp) = test_store();

    let mut fact_ids = Vec::new();
    for i in 0..20 {
        let id = store
            .add_fact(
                "general",
                &format!("compaction candidate fact {} for tombstone cleanup", i),
                None,
                None,
            )
            .await
            .unwrap();
        fact_ids.push(id);
    }

    // Delete the first 10 facts (50% tombstone ratio, above default 30% threshold).
    for id in &fact_ids[..10] {
        store.delete_fact(id).await.unwrap();
    }

    // Compact should trigger a rebuild because deleted ratio > threshold.
    store.compact_hnsw().await.unwrap();

    // Stats should show 10 remaining.
    let stats = store.stats().await.unwrap();
    assert_eq!(
        stats.total_facts, 10,
        "Should have 10 facts remaining after deleting 10"
    );

    // Search should still find the remaining facts.
    let results = store
        .search(
            "tombstone cleanup",
            Some(10),
            None,
            Some(&[SearchSourceType::Facts]),
        )
        .await
        .unwrap();
    assert!(
        !results.is_empty(),
        "Search should find remaining facts after compaction"
    );

    // Verify deleted facts are truly gone.
    for id in &fact_ids[..10] {
        let fact = store.get_fact(id).await.unwrap();
        assert!(
            fact.is_none(),
            "Deleted fact {} should not exist after compaction",
            id
        );
    }

    // Verify surviving facts are still accessible.
    for id in &fact_ids[10..] {
        let fact = store.get_fact(id).await.unwrap();
        assert!(
            fact.is_some(),
            "Surviving fact {} should still exist after compaction",
            id
        );
    }
}

/// 7.3 - Compact skips rebuild when no deletions have occurred.
#[tokio::test]
async fn compact_skips_when_healthy() {
    let (store, _tmp) = test_store();

    // Add 10 facts, delete none.
    for i in 0..10 {
        store
            .add_fact(
                "general",
                &format!("healthy fact number {} no deletions", i),
                None,
                None,
            )
            .await
            .unwrap();
    }

    // Compact should be a no-op (deleted ratio is 0%, well below threshold).
    let result = store.compact_hnsw().await;
    assert!(
        result.is_ok(),
        "compact_hnsw should succeed even when no compaction is needed"
    );

    // Verify all facts are still present.
    let stats = store.stats().await.unwrap();
    assert_eq!(
        stats.total_facts, 10,
        "All 10 facts should still be present after no-op compaction"
    );
}
