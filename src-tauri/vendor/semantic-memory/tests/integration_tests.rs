use semantic_memory::{MemoryConfig, MemoryStore, MockEmbedder, Role};
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
async fn end_to_end_add_facts_and_search() {
    let (store, _tmp) = test_store();

    // Add multiple facts
    store
        .add_fact("general", "Rust was first released in 2015", None, None)
        .await
        .unwrap();
    store
        .add_fact(
            "general",
            "Python was created by Guido van Rossum",
            None,
            None,
        )
        .await
        .unwrap();
    store
        .add_fact("general", "JavaScript was created in 10 days", None, None)
        .await
        .unwrap();
    store
        .add_fact("user", "Josh's favorite language is Rust", None, None)
        .await
        .unwrap();
    store
        .add_fact("user", "Josh works on Ironforge", None, None)
        .await
        .unwrap();

    // Search via FTS
    let results = store
        .search_fts_only("Rust", None, None, None)
        .await
        .unwrap();
    assert!(!results.is_empty(), "Should find facts about Rust");

    // Hybrid search
    let _results = store
        .search("programming languages", None, None, None)
        .await
        .unwrap();
    // With mock embedder, results are based on keyword matching (FTS) only
    // Vector similarity with mock embedder won't give semantic results
    // But the pipeline should work without errors

    // Stats
    let stats = store.stats().await.unwrap();
    assert_eq!(stats.total_facts, 5);
    assert_eq!(stats.total_sessions, 0);
}

#[tokio::test]
async fn end_to_end_document_ingestion() {
    let (store, _tmp) = test_store();

    let content = "This is a test document about machine learning. ".repeat(50);
    let doc_id = store
        .ingest_document("Test Doc", &content, "docs", Some("/test/doc.txt"), None)
        .await
        .unwrap();

    // Document should be listed
    let docs = store.list_documents("docs", 10, 0).await.unwrap();
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].title, "Test Doc");
    assert!(docs[0].chunk_count > 0);

    // Chunks should be searchable via FTS
    let results = store
        .search_fts_only("machine learning", None, None, None)
        .await
        .unwrap();
    assert!(
        !results.is_empty(),
        "Document chunks should be FTS searchable"
    );

    // Delete document
    store.delete_document(&doc_id).await.unwrap();
    let docs = store.list_documents("docs", 10, 0).await.unwrap();
    assert!(docs.is_empty());

    // FTS should be clean
    let results = store
        .search_fts_only("machine learning", None, None, None)
        .await
        .unwrap();
    assert!(
        results.is_empty(),
        "FTS should be clean after document deletion"
    );
}

#[tokio::test]
async fn clone_shares_state() {
    let (store_a, _tmp) = test_store();
    let store_b = store_a.clone();

    // Add via clone A
    store_a
        .add_fact("test", "Shared state test", None, None)
        .await
        .unwrap();

    // Find via clone B
    let results = store_b
        .search_fts_only("Shared state", None, None, None)
        .await
        .unwrap();
    assert!(!results.is_empty(), "Clone should share state");
}

#[tokio::test]
async fn conversations_and_facts_coexist() {
    let (store, _tmp) = test_store();

    // Add conversation
    let sid = store.create_session("test").await.unwrap();
    store
        .add_message(&sid, Role::User, "What is Rust?", Some(10), None)
        .await
        .unwrap();
    store
        .add_message(
            &sid,
            Role::Assistant,
            "Rust is a programming language",
            Some(15),
            None,
        )
        .await
        .unwrap();

    // Add facts
    store
        .add_fact("general", "Rust is a systems language", None, None)
        .await
        .unwrap();

    // Both should work independently
    let messages = store.get_recent_messages(&sid, 10).await.unwrap();
    assert_eq!(messages.len(), 2);

    let facts = store.list_facts("general", 10, 0).await.unwrap();
    assert_eq!(facts.len(), 1);

    let stats = store.stats().await.unwrap();
    assert_eq!(stats.total_sessions, 1);
    assert_eq!(stats.total_messages, 2);
    assert_eq!(stats.total_facts, 1);
}

#[tokio::test]
async fn reopen_database_preserves_data() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();

    // First open: add data
    {
        let config = MemoryConfig {
            base_dir: path.clone(),
            ..Default::default()
        };
        let store =
            MemoryStore::open_with_embedder(config, Box::new(MockEmbedder::new(768))).unwrap();
        store
            .add_fact("general", "Persistent fact", None, None)
            .await
            .unwrap();
        let sid = store.create_session("test").await.unwrap();
        store
            .add_message(&sid, Role::User, "Persistent message", Some(10), None)
            .await
            .unwrap();
    }

    // Second open: verify data persists
    {
        let config = MemoryConfig {
            base_dir: path,
            ..Default::default()
        };
        let store =
            MemoryStore::open_with_embedder(config, Box::new(MockEmbedder::new(768))).unwrap();

        let facts = store.list_facts("general", 10, 0).await.unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "Persistent fact");

        let sessions = store.list_sessions(10, 0).await.unwrap();
        assert_eq!(sessions.len(), 1);
    }
}

#[tokio::test]
async fn vacuum_works() {
    let (store, _tmp) = test_store();
    store
        .add_fact("general", "Vacuumable fact", None, None)
        .await
        .unwrap();
    store.vacuum().await.unwrap();
}

#[test]
fn chunk_text_exposed_on_store() {
    let (store, _tmp) = test_store();
    let chunks = store.chunk_text("Hello world, this is a test.");
    assert_eq!(chunks.len(), 1);
}

#[tokio::test]
async fn embed_exposed_on_store() {
    let (store, _tmp) = test_store();
    let embedding = store.embed("Hello world").await.unwrap();
    assert_eq!(embedding.len(), 768);
}

#[tokio::test]
async fn embed_batch_exposed_on_store() {
    let (store, _tmp) = test_store();
    let embeddings = store.embed_batch(&["Hello", "World"]).await.unwrap();
    assert_eq!(embeddings.len(), 2);
    assert_eq!(embeddings[0].len(), 768);
    assert_eq!(embeddings[1].len(), 768);
}

#[tokio::test]
async fn v2_migration_preserves_existing_data() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();

    // First open: add data (triggers V1 + V2 + V3 migrations)
    {
        let config = MemoryConfig {
            base_dir: path.clone(),
            ..Default::default()
        };
        let store =
            MemoryStore::open_with_embedder(config, Box::new(MockEmbedder::new(768))).unwrap();

        let sid = store.create_session("test").await.unwrap();
        store
            .add_message(&sid, Role::User, "V1 message", Some(10), None)
            .await
            .unwrap();
        store
            .add_fact("general", "V1 fact", None, None)
            .await
            .unwrap();
    }

    // Second open: verify data persists and V2 schema is present
    {
        let config = MemoryConfig {
            base_dir: path,
            ..Default::default()
        };
        let store =
            MemoryStore::open_with_embedder(config, Box::new(MockEmbedder::new(768))).unwrap();

        // Existing messages preserved
        let sessions = store.list_sessions(10, 0).await.unwrap();
        assert_eq!(sessions.len(), 1);
        let messages = store
            .get_recent_messages(&sessions[0].id, 10)
            .await
            .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "V1 message");

        // Facts preserved
        let facts = store.list_facts("general", 10, 0).await.unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "V1 fact");

        // V2 schema works: can add embedded messages
        let sid = &sessions[0].id;
        store
            .add_message_embedded(sid, Role::User, "V2 embedded message", Some(10), None)
            .await
            .unwrap();

        // Can search conversations
        let results = store
            .search_conversations("embedded", None, None)
            .await
            .unwrap();
        assert!(!results.is_empty());
    }
}

// ─── V2 New Tests ───────────────────────────────────────────────

#[tokio::test]
async fn test_v3_migration() {
    let (store, _tmp) = test_store();

    // V3 migration should have run at open time.
    // Verify embeddings_dirty is accessible and defaults to false.
    let dirty = store.embeddings_are_dirty().await.unwrap();
    assert!(!dirty, "Fresh DB should not have dirty embeddings");
}

#[tokio::test]
async fn test_embedding_dirty_flag() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();

    // Open store with model "model-a" at 768 dims
    {
        let config = MemoryConfig {
            base_dir: path.clone(),
            ..Default::default()
        };
        let store =
            MemoryStore::open_with_embedder(config, Box::new(MockEmbedder::new(768))).unwrap();
        store.add_fact("ns", "test fact", None, None).await.unwrap();
        assert!(!store.embeddings_are_dirty().await.unwrap());
    }

    // Reopen with different dimensions — triggers mismatch
    {
        let mut config = MemoryConfig {
            base_dir: path.clone(),
            ..Default::default()
        };
        config.embedding.dimensions = 256;
        config.embedding.model = "model-b".to_string();
        let store =
            MemoryStore::open_with_embedder(config, Box::new(MockEmbedder::new(256))).unwrap();
        assert!(
            store.embeddings_are_dirty().await.unwrap(),
            "Embeddings should be dirty after model change"
        );

        // Reembed clears the flag
        let count = store.reembed_all().await.unwrap();
        assert!(count >= 1, "Should have re-embedded at least 1 item");
        assert!(
            !store.embeddings_are_dirty().await.unwrap(),
            "Embeddings should be clean after reembed_all"
        );
    }
}

#[tokio::test]
async fn test_reembed_all_includes_messages() {
    let (store, _tmp) = test_store();
    let session = store.create_session("test").await.unwrap();

    // Add an embedded message
    store
        .add_message_embedded(&session, Role::User, "fluid dynamics", Some(5), None)
        .await
        .unwrap();

    // Add a non-embedded message (should be skipped)
    store
        .add_message(&session, Role::User, "not embedded", Some(5), None)
        .await
        .unwrap();

    // reembed_all should count the embedded message
    let count = store.reembed_all().await.unwrap();
    assert!(
        count >= 1,
        "At least the one embedded message should be re-embedded"
    );
}

#[tokio::test]
async fn test_auto_token_count() {
    let (store, _tmp) = test_store();
    let session = store.create_session("test").await.unwrap();

    // Add message with token_count = None — should auto-compute
    store
        .add_message(&session, Role::User, "hello world testing", None, None)
        .await
        .unwrap();
    let messages = store.get_recent_messages(&session, 10).await.unwrap();
    // Should have auto-computed token count (19 chars / 4 ≈ 4), not None
    assert!(messages[0].token_count.is_some());
    assert!(messages[0].token_count.unwrap() > 0);
}
