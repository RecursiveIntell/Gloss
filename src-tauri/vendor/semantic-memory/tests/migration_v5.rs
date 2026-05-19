use semantic_memory::{MemoryConfig, MemoryStore, MockEmbedder};
use tempfile::TempDir;

fn test_store_in(dir: &std::path::Path) -> MemoryStore {
    let config = MemoryConfig {
        base_dir: dir.to_path_buf(),
        ..Default::default()
    };
    let embedder = Box::new(MockEmbedder::new(768));
    MemoryStore::open_with_embedder(config, embedder).unwrap()
}

/// 8.1 - Opening a fresh MemoryStore triggers all migrations including V5.
/// Verify by adding a fact and searching (indirectly confirms the schema is valid).
#[tokio::test]
async fn v5_migration_adds_columns() {
    let tmp = TempDir::new().unwrap();
    let store = test_store_in(tmp.path());

    // Adding a fact exercises the V5 columns (embedding_q8, hnsw_keymap).
    let fact_id = store
        .add_fact(
            "general",
            "v5 migration added quantized embedding columns",
            None,
            None,
        )
        .await
        .unwrap();

    // Verify the fact is searchable (confirms schema is intact).
    let fact = store.get_fact(&fact_id).await.unwrap();
    assert!(fact.is_some(), "Fact should exist after V5 migration");

    let results = store
        .search("v5 migration quantized", Some(5), None, None)
        .await
        .unwrap();
    assert!(!results.is_empty(), "Search should work after V5 migration");
}

/// 8.2 - Reopening a MemoryStore runs migrations idempotently (no errors on second open).
#[tokio::test]
async fn v5_migration_idempotent() {
    let tmp = TempDir::new().unwrap();

    // First open: triggers all migrations.
    {
        let store = test_store_in(tmp.path());
        store
            .add_fact("general", "idempotent migration test fact", None, None)
            .await
            .unwrap();
    }

    // Second open: migrations should be a no-op (already applied).
    let store = test_store_in(tmp.path());

    // Verify the previously-added fact is still accessible.
    let results = store
        .search("idempotent migration", Some(5), None, None)
        .await
        .unwrap();
    assert!(
        !results.is_empty(),
        "Data should survive reopen; migration should be idempotent"
    );

    // Add another fact to confirm write path still works.
    let fact_id = store
        .add_fact(
            "general",
            "second fact after idempotent migration reopen",
            None,
            None,
        )
        .await
        .unwrap();

    let fact = store.get_fact(&fact_id).await.unwrap();
    assert!(
        fact.is_some(),
        "New fact should be writable after idempotent migration"
    );
}
