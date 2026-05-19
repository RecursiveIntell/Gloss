use semantic_memory::{
    MemoryConfig, MemoryStore, MockEmbedder, Role, SearchSource, SearchSourceType,
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

// ─── Test 1: Embedded message is searchable ──────────────────

#[tokio::test]
async fn embedded_message_is_searchable() {
    let (store, _tmp) = test_store();

    let session_id = store.create_session("test").await.unwrap();
    let msg_id = store
        .add_message_embedded(
            &session_id,
            Role::User,
            "The Navier-Stokes equations govern fluid dynamics",
            Some(10),
            None,
        )
        .await
        .unwrap();

    let results = store
        .search_conversations("fluid dynamics", None, None)
        .await
        .unwrap();

    assert_eq!(results.len(), 1, "Should find 1 embedded message");
    assert!(results[0].content.contains("fluid dynamics"));

    // Verify source is Message with correct IDs
    match &results[0].source {
        SearchSource::Message {
            message_id,
            session_id: sid,
            ..
        } => {
            assert_eq!(*message_id, msg_id);
            assert_eq!(sid, &session_id);
        }
        other => panic!("Expected Message source, got {:?}", other),
    }
}

// ─── Test 2: Plain messages are searchable via FTS ──────────

#[tokio::test]
async fn non_embedded_message_is_searchable_via_fts() {
    let (store, _tmp) = test_store();

    let session_id = store.create_session("test").await.unwrap();
    store
        .add_message(
            &session_id,
            Role::User,
            "invisible message about quantum computing",
            Some(10),
            None,
        )
        .await
        .unwrap();

    let results = store
        .search_conversations("quantum computing", None, None)
        .await
        .unwrap();

    assert_eq!(
        results.len(),
        1,
        "Plain messages should be searchable via FTS"
    );
    assert!(results[0].content.contains("quantum computing"));
}

// ─── Test 3: Session ID filtering ───────────────────────────

#[tokio::test]
async fn session_id_filtering() {
    let (store, _tmp) = test_store();

    let session_a = store.create_session("test").await.unwrap();
    let session_b = store.create_session("test").await.unwrap();

    store
        .add_message_embedded(
            &session_a,
            Role::User,
            "Session A message about neural networks",
            Some(10),
            None,
        )
        .await
        .unwrap();

    store
        .add_message_embedded(
            &session_b,
            Role::User,
            "Session B message about neural networks",
            Some(10),
            None,
        )
        .await
        .unwrap();

    // Filter to session A only
    let results = store
        .search_conversations("neural networks", None, Some(&[session_a.as_str()]))
        .await
        .unwrap();

    assert_eq!(results.len(), 1, "Should only find session A results");
    match &results[0].source {
        SearchSource::Message { session_id, .. } => {
            assert_eq!(session_id, &session_a);
        }
        other => panic!("Expected Message source, got {:?}", other),
    }
}

// ─── Test 4: Mixed source type search ───────────────────────

#[tokio::test]
async fn mixed_source_type_search() {
    let (store, _tmp) = test_store();

    // Add a fact
    store
        .add_fact(
            "general",
            "Machine learning uses gradient descent",
            None,
            None,
        )
        .await
        .unwrap();

    // Add an embedded message with related content
    let session_id = store.create_session("test").await.unwrap();
    store
        .add_message_embedded(
            &session_id,
            Role::User,
            "Machine learning requires large datasets for training",
            Some(10),
            None,
        )
        .await
        .unwrap();

    // Search with both Facts and Messages source types — should find both
    let results = store
        .search(
            "machine learning",
            None,
            None,
            Some(&[SearchSourceType::Facts, SearchSourceType::Messages]),
        )
        .await
        .unwrap();

    let has_fact = results
        .iter()
        .any(|r| matches!(&r.source, SearchSource::Fact { .. }));
    let has_message = results
        .iter()
        .any(|r| matches!(&r.source, SearchSource::Message { .. }));
    assert!(has_fact, "Should include fact results");
    assert!(has_message, "Should include message results");

    // Default search (no source_types) should NOT include messages
    let results = store
        .search("machine learning", None, None, None)
        .await
        .unwrap();
    let has_message = results
        .iter()
        .any(|r| matches!(&r.source, SearchSource::Message { .. }));
    assert!(!has_message, "Default search should NOT include messages");
}

// ─── Test 5: Message FTS delete on session delete ───────────

#[tokio::test]
async fn message_fts_cleanup_on_session_delete() {
    let (store, _tmp) = test_store();

    let session_id = store.create_session("test").await.unwrap();
    store
        .add_message_embedded(
            &session_id,
            Role::User,
            "Ephemeral message about photosynthesis",
            Some(10),
            None,
        )
        .await
        .unwrap();

    // Verify message is searchable
    let results = store
        .search_conversations("photosynthesis", None, None)
        .await
        .unwrap();
    assert_eq!(results.len(), 1, "Should find message before deletion");

    // Delete session
    store.delete_session(&session_id).await.unwrap();

    // Search should return nothing — no ghost FTS entries
    let results = store
        .search_conversations("photosynthesis", None, None)
        .await
        .unwrap();
    assert_eq!(
        results.len(),
        0,
        "Should find nothing after session deletion — no ghost FTS entries"
    );
}
