//! HNSW persistence tests: keymap survival across reopen, deletion persistence,
//! explicit flush, rebuild, and graceful reopen.
//!
//! Only compiled when the `hnsw` feature is enabled.

#![cfg(feature = "hnsw")]

use semantic_memory::{
    MemoryConfig, MemoryStore, MockEmbedder, SearchSource, SearchSourceType, StoragePaths,
    VerifyMode,
};
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

fn reopen_store(base_dir: &std::path::Path) -> MemoryStore {
    let config = MemoryConfig {
        base_dir: base_dir.to_path_buf(),
        ..Default::default()
    };
    let embedder = Box::new(MockEmbedder::new(768));
    MemoryStore::open_with_embedder(config, embedder).unwrap()
}

/// 2.1 — After adding facts, closing, and reopening the store on the same
/// directory, FTS search should still find the persisted facts with correct IDs.
#[tokio::test]
async fn keymap_survives_reopen() {
    let tmp = TempDir::new().unwrap();
    let base_dir = tmp.path().to_path_buf();

    // Collect fact IDs so we can verify one after reopen.
    let mut fact_ids = Vec::new();

    // First open: add 10 facts.
    {
        let store = reopen_store(&base_dir);
        for i in 0..10 {
            let id = store
                .add_fact(
                    "persist",
                    &format!("Persistence fact number {}", i),
                    None,
                    None,
                )
                .await
                .unwrap();
            fact_ids.push(id);
        }
    } // store dropped — should flush to disk

    // Second open: search for one of the facts.
    {
        let store = reopen_store(&base_dir);

        let results = store
            .search_fts_only(
                "Persistence fact number 7",
                Some(10),
                None,
                Some(&[SearchSourceType::Facts]),
            )
            .await
            .unwrap();

        assert!(
            !results.is_empty(),
            "Should find persisted fact after reopen"
        );

        // Verify the correct fact_id is present in results.
        let found_ids: Vec<&str> = results
            .iter()
            .filter_map(|r| match &r.source {
                SearchSource::Fact { fact_id, .. } => Some(fact_id.as_str()),
                _ => None,
            })
            .collect();

        assert!(
            found_ids.contains(&fact_ids[7].as_str()),
            "Expected fact_id {} in results, got {:?}",
            fact_ids[7],
            found_ids
        );
    }
}

/// 2.2 — After adding 10 facts, deleting 3, closing, and reopening, the deleted
/// facts should not appear in search while the remaining ones should.
#[tokio::test]
async fn deletions_survive_reopen() {
    let tmp = TempDir::new().unwrap();
    let base_dir = tmp.path().to_path_buf();

    let mut fact_ids = Vec::new();

    // First open: add 10 facts, delete 3.
    {
        let store = reopen_store(&base_dir);

        for i in 0..10 {
            let id = store
                .add_fact(
                    "deletion",
                    &format!("Deletable fact item {}", i),
                    None,
                    None,
                )
                .await
                .unwrap();
            fact_ids.push(id);
        }

        // Delete facts 0, 1, 2.
        for id in &fact_ids[0..3] {
            store.delete_fact(id).await.unwrap();
        }
    } // store dropped

    // Second open: verify deletions persisted.
    {
        let store = reopen_store(&base_dir);

        // Deleted facts should NOT appear.
        for (i, _fact_id) in fact_ids.iter().enumerate().take(3) {
            let query = format!("Deletable fact item {}", i);
            let results = store
                .search_fts_only(&query, Some(10), None, Some(&[SearchSourceType::Facts]))
                .await
                .unwrap();

            let has_deleted_id = results.iter().any(|r| match &r.source {
                SearchSource::Fact { fact_id, .. } => fact_id == &fact_ids[i],
                _ => false,
            });

            assert!(
                !has_deleted_id,
                "Deleted fact {} should not appear after reopen",
                fact_ids[i]
            );
        }

        // Remaining facts (3..10) should still be findable.
        let results = store
            .search_fts_only(
                "Deletable fact item",
                Some(20),
                None,
                Some(&[SearchSourceType::Facts]),
            )
            .await
            .unwrap();

        assert_eq!(
            results.len(),
            7,
            "Should have exactly 7 remaining facts after deleting 3 of 10"
        );
    }
}

/// 2.3 — After adding facts and calling `flush_hnsw()` explicitly, the facts
/// should remain findable via search.
#[tokio::test]
async fn keymap_flush_on_explicit_flush() {
    let (store, _tmp) = test_store();

    let mut fact_ids = Vec::new();
    for i in 0..5 {
        let id = store
            .add_fact("flush", &format!("Flushed fact entry {}", i), None, None)
            .await
            .unwrap();
        fact_ids.push(id);
    }

    // Explicitly flush HNSW to persist graph + keymap.
    store.flush_hnsw().unwrap();

    // Verify all facts are still findable via FTS after flush.
    let results = store
        .search_fts_only(
            "Flushed fact entry",
            Some(10),
            None,
            Some(&[SearchSourceType::Facts]),
        )
        .await
        .unwrap();

    assert_eq!(
        results.len(),
        5,
        "All 5 facts should be findable after explicit flush_hnsw()"
    );

    // Verify each fact_id appears in results.
    let result_ids: Vec<String> = results
        .iter()
        .filter_map(|r| match &r.source {
            SearchSource::Fact { fact_id, .. } => Some(fact_id.clone()),
            _ => None,
        })
        .collect();

    for id in &fact_ids {
        assert!(
            result_ids.contains(id),
            "Fact {} should be present in search results after flush",
            id
        );
    }
}

/// 2.4 — After adding facts and calling `rebuild_hnsw_index()`, search should
/// still work both before and after a close/reopen cycle.
#[tokio::test]
async fn rebuild_preserves_keymap() {
    let tmp = TempDir::new().unwrap();
    let base_dir = tmp.path().to_path_buf();

    let mut fact_ids = Vec::new();

    // First open: add facts, rebuild, verify search works.
    {
        let store = reopen_store(&base_dir);

        for i in 0..8 {
            let id = store
                .add_fact("rebuild", &format!("Rebuild test fact {}", i), None, None)
                .await
                .unwrap();
            fact_ids.push(id);
        }

        // Rebuild HNSW index from SQLite embeddings.
        store.rebuild_hnsw_index().await.unwrap();

        // Search should still work after rebuild.
        let results = store
            .search_fts_only(
                "Rebuild test fact",
                Some(10),
                None,
                Some(&[SearchSourceType::Facts]),
            )
            .await
            .unwrap();

        assert_eq!(
            results.len(),
            8,
            "All 8 facts should be findable after rebuild_hnsw_index()"
        );
    } // store dropped

    // Second open: search should still work after reopen.
    {
        let store = reopen_store(&base_dir);

        let results = store
            .search_fts_only(
                "Rebuild test fact",
                Some(10),
                None,
                Some(&[SearchSourceType::Facts]),
            )
            .await
            .unwrap();

        assert_eq!(
            results.len(),
            8,
            "All 8 facts should be findable after reopen post-rebuild"
        );

        // Verify a specific fact_id.
        let found_ids: Vec<String> = results
            .iter()
            .filter_map(|r| match &r.source {
                SearchSource::Fact { fact_id, .. } => Some(fact_id.clone()),
                _ => None,
            })
            .collect();

        assert!(
            found_ids.contains(&fact_ids[4]),
            "Fact {} should survive rebuild + reopen cycle",
            fact_ids[4]
        );
    }
}

/// 2.5 — Opening a store on a fresh directory, adding facts, closing, then
/// reopening should not crash regardless of keymap table state.
#[tokio::test]
async fn reopen_with_no_keymap_table_graceful() {
    let tmp = TempDir::new().unwrap();
    let base_dir = tmp.path().to_path_buf();

    // First open: add facts.
    {
        let store = reopen_store(&base_dir);

        for i in 0..3 {
            store
                .add_fact(
                    "graceful",
                    &format!("Graceful reopen fact {}", i),
                    None,
                    None,
                )
                .await
                .unwrap();
        }
    } // store dropped

    // Second open: should not panic or error.
    let store = reopen_store(&base_dir);

    // Verify we can still search (data survived).
    let results = store
        .search_fts_only(
            "Graceful reopen fact",
            Some(10),
            None,
            Some(&[SearchSourceType::Facts]),
        )
        .await
        .unwrap();

    assert_eq!(
        results.len(),
        3,
        "All 3 facts should survive a close/reopen cycle"
    );
}

fn fixed_embedding(seed: f32) -> Vec<f32> {
    let mut embedding = vec![0.0; 768];
    embedding[0] = seed;
    embedding[1] = 1.0 - seed;
    embedding
}

#[cfg(feature = "testing")]
#[tokio::test]
async fn full_integrity_reports_missing_live_hnsw_key() {
    let (store, _tmp) = test_store();
    let fact_id = store
        .add_fact_with_embedding(
            "integrity",
            "missing key target",
            &fixed_embedding(0.25),
            None,
            None,
        )
        .await
        .unwrap();
    store.flush_hnsw().unwrap();

    store
        .raw_execute(
            "DELETE FROM hnsw_keymap WHERE item_key = ?1",
            vec![format!("fact:{fact_id}")],
        )
        .await
        .unwrap();

    let report = store.verify_integrity(VerifyMode::Full).await.unwrap();
    assert!(report.issues.iter().any(|issue| {
        issue.contains("HNSW keymap missing live embedded SQLite row")
            && issue.contains(&format!("fact:{fact_id}"))
    }));
}

#[cfg(feature = "testing")]
#[tokio::test]
async fn full_integrity_reports_stale_or_wrong_domain_hnsw_key() {
    let (store, _tmp) = test_store();
    let fact_id = store
        .add_fact_with_embedding(
            "integrity",
            "wrong domain target",
            &fixed_embedding(0.35),
            None,
            None,
        )
        .await
        .unwrap();
    store.flush_hnsw().unwrap();

    store
        .raw_execute(
            "UPDATE hnsw_keymap SET item_key = ?1 WHERE item_key = ?2",
            vec!["bogus:not-live".to_string(), format!("fact:{fact_id}")],
        )
        .await
        .unwrap();

    let report = store.verify_integrity(VerifyMode::Full).await.unwrap();
    assert!(report.issues.iter().any(|issue| {
        issue.contains("unsupported key domain") && issue.contains("bogus:not-live")
    }));
    assert!(report.issues.iter().any(|issue| {
        issue.contains("HNSW keymap missing live embedded SQLite row")
            && issue.contains(&format!("fact:{fact_id}"))
    }));
}

#[cfg(feature = "testing")]
#[tokio::test]
async fn full_integrity_reports_stale_valid_domain_hnsw_key() {
    let (store, _tmp) = test_store();
    let fact_id = store
        .add_fact_with_embedding(
            "integrity",
            "stale valid domain target",
            &fixed_embedding(0.55),
            None,
            None,
        )
        .await
        .unwrap();
    store.flush_hnsw().unwrap();

    store
        .raw_execute(
            "UPDATE hnsw_keymap SET item_key = ?1 WHERE item_key = ?2",
            vec!["fact:not-live".to_string(), format!("fact:{fact_id}")],
        )
        .await
        .unwrap();

    let report = store.verify_integrity(VerifyMode::Full).await.unwrap();
    assert!(report.issues.iter().any(|issue| {
        issue.contains("stale active entry without live embedded SQLite row")
            && issue.contains("fact:not-live")
    }));
    assert!(report.issues.iter().any(|issue| {
        issue.contains("HNSW keymap missing live embedded SQLite row")
            && issue.contains(&format!("fact:{fact_id}"))
    }));
}

#[cfg(feature = "testing")]
#[tokio::test]
async fn full_integrity_catches_swapped_hnsw_key_ids_when_counts_match() {
    let (store, _tmp) = test_store();
    let first_id = store
        .add_fact_with_embedding(
            "integrity",
            "first swapped vector target",
            &fixed_embedding(0.10),
            None,
            None,
        )
        .await
        .unwrap();
    let second_id = store
        .add_fact_with_embedding(
            "integrity",
            "second swapped vector target",
            &fixed_embedding(0.90),
            None,
            None,
        )
        .await
        .unwrap();
    store.flush_hnsw().unwrap();

    let first_key = format!("fact:{first_id}");
    let second_key = format!("fact:{second_id}");
    let temporary_key = format!("fact:{first_id}-swap-temp");
    store
        .raw_execute(
            "UPDATE hnsw_keymap SET item_key = ?1 WHERE item_key = ?2",
            vec![temporary_key.clone(), first_key.clone()],
        )
        .await
        .unwrap();
    store
        .raw_execute(
            "UPDATE hnsw_keymap SET item_key = ?1 WHERE item_key = ?2",
            vec![first_key.clone(), second_key.clone()],
        )
        .await
        .unwrap();
    store
        .raw_execute(
            "UPDATE hnsw_keymap SET item_key = ?1 WHERE item_key = ?2",
            vec![second_key.clone(), temporary_key],
        )
        .await
        .unwrap();

    let report = store.verify_integrity(VerifyMode::Full).await.unwrap();
    assert!(
        report.issues.iter().any(|issue| {
            issue.contains("vector does not match the authoritative SQLite embedding")
                && (issue.contains(&first_key) || issue.contains(&second_key))
        }),
        "expected swapped key/vector mismatch, got {:?}",
        report.issues
    );
    assert!(
        !report
            .issues
            .iter()
            .any(|issue| issue.contains("HNSW keymap drift:")),
        "count parity should remain intact for this corruption: {:?}",
        report.issues
    );
}

#[tokio::test]
async fn unsupported_hnsw_sidecar_version_rebuilds_from_sqlite_on_reopen() {
    let tmp = TempDir::new().unwrap();
    let base_dir = tmp.path().to_path_buf();
    let fact_id;

    {
        let store = reopen_store(&base_dir);
        fact_id = store
            .add_fact_with_embedding(
                "integrity",
                "unsupported sidecar version target",
                &fixed_embedding(0.45),
                None,
                None,
            )
            .await
            .unwrap();
        store.flush_hnsw().unwrap();
    }

    let paths = StoragePaths::new(&base_dir);
    let mut graph_bytes = std::fs::read(paths.hnsw_graph_path()).unwrap();
    graph_bytes[4..6].copy_from_slice(&99u16.to_le_bytes());
    std::fs::write(paths.hnsw_graph_path(), graph_bytes).unwrap();

    let store = reopen_store(&base_dir);
    let report = store.verify_integrity(VerifyMode::Full).await.unwrap();
    assert!(
        report.ok,
        "reopen should rebuild unsupported HNSW sidecar from SQLite: {:?}",
        report.issues
    );

    let results = store
        .search_fts_only(
            "unsupported sidecar version target",
            Some(5),
            None,
            Some(&[SearchSourceType::Facts]),
        )
        .await
        .unwrap();
    assert!(results.iter().any(|result| {
        matches!(
            &result.source,
            SearchSource::Fact { fact_id: found, .. } if found == &fact_id
        )
    }));
}

#[tokio::test]
async fn wrong_hnsw_sidecar_dimension_rebuilds_from_sqlite_on_reopen() {
    let tmp = TempDir::new().unwrap();
    let base_dir = tmp.path().to_path_buf();

    {
        let store = reopen_store(&base_dir);
        store
            .add_fact_with_embedding(
                "integrity",
                "wrong sidecar dimension target",
                &fixed_embedding(0.65),
                None,
                None,
            )
            .await
            .unwrap();
        store.flush_hnsw().unwrap();
    }

    let paths = StoragePaths::new(&base_dir);
    let mut data_bytes = std::fs::read(paths.hnsw_data_path()).unwrap();
    data_bytes[8..12].copy_from_slice(&123u32.to_le_bytes());
    std::fs::write(paths.hnsw_data_path(), data_bytes).unwrap();

    let store = reopen_store(&base_dir);
    let report = store.verify_integrity(VerifyMode::Full).await.unwrap();
    assert!(
        report.ok,
        "reopen should rebuild wrong-dimension HNSW sidecar from SQLite: {:?}",
        report.issues
    );
}

#[tokio::test]
async fn hnsw_manifest_written_after_graph_and_data() {
    let tmp = TempDir::new().unwrap();
    let base_dir = tmp.path().to_path_buf();
    {
        let store = reopen_store(&base_dir);
        store
            .add_fact_with_embedding(
                "integrity",
                "manifest written target",
                &fixed_embedding(0.42),
                None,
                None,
            )
            .await
            .unwrap();
        store.flush_hnsw().unwrap();
    }

    let paths = StoragePaths::new(&base_dir);
    assert!(paths.hnsw_graph_path().exists());
    assert!(paths.hnsw_data_path().exists());
    assert!(paths.hnsw_manifest_path().exists());

    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(paths.hnsw_manifest_path()).unwrap()).unwrap();
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["basename"], "memory");
    assert_eq!(manifest["graph_file_name"], "memory.hnsw.graph");
    assert_eq!(manifest["data_file_name"], "memory.hnsw.data");
    assert_eq!(manifest["dimensions"], 768);
    assert_eq!(manifest["vector_count"], 1);
    assert!(manifest["graph_digest"]
        .as_str()
        .unwrap()
        .starts_with("blake3:"));
    assert!(manifest["data_digest"]
        .as_str()
        .unwrap()
        .starts_with("blake3:"));
}

#[tokio::test]
async fn hnsw_manifest_graph_digest_mismatch_rebuilds_from_sqlite() {
    let tmp = TempDir::new().unwrap();
    let base_dir = tmp.path().to_path_buf();
    let fact_id;
    {
        let store = reopen_store(&base_dir);
        fact_id = store
            .add_fact_with_embedding(
                "integrity",
                "graph digest mismatch target",
                &fixed_embedding(0.11),
                None,
                None,
            )
            .await
            .unwrap();
        store.flush_hnsw().unwrap();
    }

    let paths = StoragePaths::new(&base_dir);
    let mut graph = std::fs::read(paths.hnsw_graph_path()).unwrap();
    graph.push(0xff);
    std::fs::write(paths.hnsw_graph_path(), graph).unwrap();

    let store = reopen_store(&base_dir);
    let report = store.verify_integrity(VerifyMode::Full).await.unwrap();
    assert!(
        report.ok,
        "graph digest mismatch should rebuild from SQLite: {:?}",
        report.issues
    );
    let results = store
        .search_fts_only(
            "graph digest mismatch target",
            Some(5),
            None,
            Some(&[SearchSourceType::Facts]),
        )
        .await
        .unwrap();
    assert!(results.iter().any(|result| {
        matches!(
            &result.source,
            SearchSource::Fact { fact_id: found, .. } if found == &fact_id
        )
    }));
}

#[tokio::test]
async fn hnsw_manifest_data_digest_mismatch_rebuilds_from_sqlite() {
    let tmp = TempDir::new().unwrap();
    let base_dir = tmp.path().to_path_buf();
    {
        let store = reopen_store(&base_dir);
        store
            .add_fact_with_embedding(
                "integrity",
                "data digest mismatch target",
                &fixed_embedding(0.21),
                None,
                None,
            )
            .await
            .unwrap();
        store.flush_hnsw().unwrap();
    }

    let paths = StoragePaths::new(&base_dir);
    let mut data = std::fs::read(paths.hnsw_data_path()).unwrap();
    let last = data.len().saturating_sub(1);
    data[last] ^= 0xff;
    std::fs::write(paths.hnsw_data_path(), data).unwrap();

    let store = reopen_store(&base_dir);
    let report = store.verify_integrity(VerifyMode::Full).await.unwrap();
    assert!(
        report.ok,
        "data digest mismatch should rebuild from SQLite: {:?}",
        report.issues
    );
}

#[tokio::test]
async fn hnsw_manifest_points_to_missing_file_rebuilds_from_sqlite() {
    let tmp = TempDir::new().unwrap();
    let base_dir = tmp.path().to_path_buf();
    {
        let store = reopen_store(&base_dir);
        store
            .add_fact_with_embedding(
                "integrity",
                "manifest missing file target",
                &fixed_embedding(0.31),
                None,
                None,
            )
            .await
            .unwrap();
        store.flush_hnsw().unwrap();
    }

    let paths = StoragePaths::new(&base_dir);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(paths.hnsw_manifest_path()).unwrap()).unwrap();
    manifest["data_file_name"] = serde_json::Value::String("missing.hnsw.data".to_string());
    std::fs::write(
        paths.hnsw_manifest_path(),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let store = reopen_store(&base_dir);
    let report = store.verify_integrity(VerifyMode::Full).await.unwrap();
    assert!(
        report.ok,
        "manifest file mismatch should rebuild from SQLite: {:?}",
        report.issues
    );
}

#[tokio::test]
async fn legacy_hnsw_sidecar_without_manifest_loads_deterministically() {
    let tmp = TempDir::new().unwrap();
    let base_dir = tmp.path().to_path_buf();
    {
        let store = reopen_store(&base_dir);
        store
            .add_fact_with_embedding(
                "integrity",
                "legacy no manifest target",
                &fixed_embedding(0.41),
                None,
                None,
            )
            .await
            .unwrap();
        store.flush_hnsw().unwrap();
    }

    let paths = StoragePaths::new(&base_dir);
    std::fs::remove_file(paths.hnsw_manifest_path()).unwrap();

    let store = reopen_store(&base_dir);
    let report = store.verify_integrity(VerifyMode::Full).await.unwrap();
    assert!(
        report.ok,
        "legacy no-manifest sidecar should load or rebuild deterministically: {:?}",
        report.issues
    );
}
