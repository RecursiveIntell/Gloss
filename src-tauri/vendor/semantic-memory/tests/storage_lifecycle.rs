//! Storage lifecycle tests: persistence, reopen, and HNSW rebuild.
//!
//! Only compiled when the `hnsw` feature is enabled.

#![cfg(feature = "hnsw")]

use semantic_memory::{MemoryConfig, MemoryStore, MockEmbedder};
use tempfile::TempDir;

#[tokio::test]
async fn hnsw_persist_and_reload() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();

    // Create and populate
    {
        let config = MemoryConfig {
            base_dir: path.clone(),
            ..Default::default()
        };
        let store =
            MemoryStore::open_with_embedder(config, Box::new(MockEmbedder::new(768))).unwrap();

        store
            .add_fact("general", "Persistent fact alpha", None, None)
            .await
            .unwrap();
        store
            .add_fact("general", "Persistent fact beta", None, None)
            .await
            .unwrap();

        // Explicitly flush
        store.flush_hnsw().unwrap();
    }

    // Reopen and verify
    {
        let config = MemoryConfig {
            base_dir: path,
            ..Default::default()
        };
        let store =
            MemoryStore::open_with_embedder(config, Box::new(MockEmbedder::new(768))).unwrap();

        // Facts should persist (SQLite)
        let facts = store.list_facts("general", 10, 0).await.unwrap();
        assert_eq!(facts.len(), 2);

        // FTS search should still work
        let results = store
            .search_fts_only("Persistent alpha", Some(5), None, None)
            .await
            .unwrap();
        assert!(!results.is_empty(), "FTS should work after reopen");

        // Hybrid search should work (HNSW is either loaded or auto-created)
        let results = store
            .search("Persistent fact", Some(5), None, None)
            .await
            .unwrap();
        assert!(
            !results.is_empty(),
            "Hybrid search should work after reopen"
        );
    }
}

#[tokio::test]
async fn hnsw_sidecar_deleted_triggers_rebuild() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();

    // Create and populate
    {
        let config = MemoryConfig {
            base_dir: path.clone(),
            ..Default::default()
        };
        let store =
            MemoryStore::open_with_embedder(config, Box::new(MockEmbedder::new(768))).unwrap();

        for i in 0..10 {
            store
                .add_fact("ns", &format!("Rebuild test fact number {}", i), None, None)
                .await
                .unwrap();
        }
        store.flush_hnsw().unwrap();
    }

    // Delete HNSW sidecar files
    let graph_path = path.join("memory.hnsw.graph");
    let data_path = path.join("memory.hnsw.data");
    if graph_path.exists() {
        std::fs::remove_file(&graph_path).unwrap();
    }
    if data_path.exists() {
        std::fs::remove_file(&data_path).unwrap();
    }

    // Reopen — should auto-create a new empty HNSW (no sidecar files)
    {
        let config = MemoryConfig {
            base_dir: path,
            ..Default::default()
        };
        let store =
            MemoryStore::open_with_embedder(config, Box::new(MockEmbedder::new(768))).unwrap();

        // SQLite data should persist regardless
        let facts = store.list_facts("ns", 20, 0).await.unwrap();
        assert_eq!(
            facts.len(),
            10,
            "SQLite facts should survive sidecar deletion"
        );

        // FTS should still work (independent of HNSW)
        let results = store
            .search_fts_only("Rebuild test fact", Some(5), None, None)
            .await
            .unwrap();
        assert!(
            !results.is_empty(),
            "FTS should work even without HNSW sidecar"
        );

        // Rebuild HNSW from SQLite
        store.rebuild_hnsw_index().await.unwrap();

        // Hybrid search should now work
        let results = store
            .search("test fact", Some(5), None, None)
            .await
            .unwrap();
        assert!(
            !results.is_empty(),
            "Search should work after HNSW rebuild from SQLite"
        );
    }
}

#[tokio::test]
async fn hnsw_files_created_on_flush() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();

    let config = MemoryConfig {
        base_dir: path.clone(),
        ..Default::default()
    };
    let store = MemoryStore::open_with_embedder(config, Box::new(MockEmbedder::new(768))).unwrap();

    // Add a fact so HNSW has something to save
    store
        .add_fact("ns", "Fact for persistence check", None, None)
        .await
        .unwrap();

    // Flush creates sidecar files
    store.flush_hnsw().unwrap();

    let graph_path = path.join("memory.hnsw.graph");
    let data_path = path.join("memory.hnsw.data");

    assert!(
        graph_path.exists(),
        "HNSW graph file should exist after flush"
    );
    assert!(
        data_path.exists(),
        "HNSW data file should exist after flush"
    );
}
