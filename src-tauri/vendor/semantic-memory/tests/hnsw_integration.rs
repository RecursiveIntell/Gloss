//! HNSW integration tests: full pipeline from insert through search.
//!
//! Only compiled when the `hnsw` feature is enabled.

#![cfg(feature = "hnsw")]

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

#[tokio::test]
async fn hnsw_insert_and_search_facts() {
    let (store, _tmp) = test_store();

    store
        .add_fact("science", "The Earth orbits the Sun", None, None)
        .await
        .unwrap();
    store
        .add_fact("science", "Water boils at 100 degrees Celsius", None, None)
        .await
        .unwrap();
    store
        .add_fact("personal", "My favorite color is blue", None, None)
        .await
        .unwrap();

    // Search should return results
    let results = store
        .search("Earth orbit", Some(5), None, None)
        .await
        .unwrap();
    assert!(
        !results.is_empty(),
        "Should find facts via HNSW-backed search"
    );
}

#[tokio::test]
async fn hnsw_multi_domain_search() {
    let (store, _tmp) = test_store();

    // Add facts — use "programming" so FTS will match
    store
        .add_fact(
            "general",
            "Rust programming is great for systems",
            None,
            None,
        )
        .await
        .unwrap();

    // Add document chunks — also contain "programming"
    let content = "Programming in Rust requires understanding ownership and borrowing. ".repeat(20);
    store
        .ingest_document("Rust Guide", &content, "docs", None, None)
        .await
        .unwrap();

    // Add embedded message
    let sid = store.create_session("test").await.unwrap();
    store
        .add_message_embedded(
            &sid,
            Role::User,
            "Tell me about Rust programming",
            None,
            None,
        )
        .await
        .unwrap();

    // Search across facts + chunks (messages excluded by default)
    let results = store
        .search("programming", Some(10), None, None)
        .await
        .unwrap();
    assert!(
        !results.is_empty(),
        "Multi-domain search should return results"
    );

    // Verify we get results from multiple source types
    let has_fact = results
        .iter()
        .any(|r| matches!(r.source, SearchSource::Fact { .. }));
    let has_chunk = results
        .iter()
        .any(|r| matches!(r.source, SearchSource::Chunk { .. }));
    assert!(has_fact, "Should find facts in multi-domain search");
    assert!(has_chunk, "Should find chunks in multi-domain search");

    // Search messages only via dedicated API
    let msg_results = store
        .search_conversations("Rust", Some(5), None)
        .await
        .unwrap();
    // Messages found via FTS at minimum
    assert!(
        !msg_results.is_empty(),
        "Message search should find embedded messages"
    );
}

#[tokio::test]
async fn hnsw_namespace_filtering() {
    let (store, _tmp) = test_store();

    store
        .add_fact("science", "Physics studies matter and energy", None, None)
        .await
        .unwrap();
    store
        .add_fact("cooking", "Bread requires yeast to rise", None, None)
        .await
        .unwrap();

    // Search only in "science" namespace
    let results = store
        .search("studies", Some(5), Some(&["science"]), None)
        .await
        .unwrap();
    // FTS should find the science fact
    for r in &results {
        if let semantic_memory::SearchSource::Fact { namespace, .. } = &r.source {
            assert_eq!(namespace, "science", "Should only return science namespace");
        }
    }
}

#[tokio::test]
async fn hnsw_search_empty_store() {
    let (store, _tmp) = test_store();

    let results = store.search("anything", Some(5), None, None).await.unwrap();
    assert!(results.is_empty(), "Empty store should return no results");
}

#[tokio::test]
async fn hnsw_delete_removes_from_search() {
    let (store, _tmp) = test_store();

    let fact_id = store
        .add_fact("general", "Temporary fact to delete", None, None)
        .await
        .unwrap();

    // Verify it's searchable
    let results = store
        .search_fts_only("Temporary", Some(5), None, None)
        .await
        .unwrap();
    assert!(
        !results.is_empty(),
        "Fact should be searchable before delete"
    );

    // Delete it
    store.delete_fact(&fact_id).await.unwrap();

    // Should no longer appear
    let results = store
        .search_fts_only("Temporary", Some(5), None, None)
        .await
        .unwrap();
    assert!(
        results.is_empty(),
        "Deleted fact should not appear in search results"
    );
}

#[tokio::test]
async fn hnsw_source_type_filtering() {
    let (store, _tmp) = test_store();

    // Add a fact and a document
    store
        .add_fact("general", "Important fact about testing", None, None)
        .await
        .unwrap();
    let content = "Document about testing procedures and quality assurance. ".repeat(20);
    store
        .ingest_document("Test Doc", &content, "docs", None, None)
        .await
        .unwrap();

    // Search only facts
    let fact_results = store
        .search("testing", Some(10), None, Some(&[SearchSourceType::Facts]))
        .await
        .unwrap();
    for r in &fact_results {
        assert!(
            matches!(r.source, semantic_memory::SearchSource::Fact { .. }),
            "Should only return facts"
        );
    }

    // Search only chunks
    let chunk_results = store
        .search("testing", Some(10), None, Some(&[SearchSourceType::Chunks]))
        .await
        .unwrap();
    for r in &chunk_results {
        assert!(
            matches!(r.source, semantic_memory::SearchSource::Chunk { .. }),
            "Should only return chunks"
        );
    }
}

#[tokio::test]
async fn hnsw_document_delete_cleans_up() {
    let (store, _tmp) = test_store();

    let content = "Unique document content for deletion test purposes. ".repeat(20);
    let doc_id = store
        .ingest_document("Delete Doc", &content, "docs", None, None)
        .await
        .unwrap();

    // Verify searchable
    let results = store
        .search_fts_only("Unique document deletion", Some(5), None, None)
        .await
        .unwrap();
    assert!(!results.is_empty(), "Document should be searchable");

    // Delete
    store.delete_document(&doc_id).await.unwrap();

    // Should be gone
    let results = store
        .search_fts_only("Unique document deletion", Some(5), None, None)
        .await
        .unwrap();
    assert!(
        results.is_empty(),
        "Deleted document chunks should not appear"
    );

    // Stats should reflect deletion
    let stats = store.stats().await.unwrap();
    assert_eq!(stats.total_documents, 0);
    assert_eq!(stats.total_chunks, 0);
}

// ── LIB-LOW-002: NaN/infinity rejection ────────────────────────

#[test]
fn hnsw_insert_nan_is_rejected() {
    let config = semantic_memory::HnswConfig {
        dimensions: 4,
        max_elements: 10,
        ..Default::default()
    };
    let index = semantic_memory::HnswIndex::new(config).unwrap();

    let nan_vector = vec![1.0, f32::NAN, 0.0, 0.5];
    let result = index.insert("nan-key".into(), &nan_vector);
    assert!(result.is_err(), "NaN vector must be rejected");
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("NaN") || msg.contains("infinity"),
        "error should mention NaN/infinity: {msg}"
    );
}

#[test]
fn hnsw_insert_infinity_is_rejected() {
    let config = semantic_memory::HnswConfig {
        dimensions: 4,
        max_elements: 10,
        ..Default::default()
    };
    let index = semantic_memory::HnswIndex::new(config).unwrap();

    let inf_vector = vec![1.0, 0.0, f32::INFINITY, 0.5];
    let result = index.insert("inf-key".into(), &inf_vector);
    assert!(result.is_err(), "infinity vector must be rejected");
}

#[test]
fn hnsw_search_nan_is_rejected() {
    let config = semantic_memory::HnswConfig {
        dimensions: 4,
        max_elements: 10,
        ..Default::default()
    };
    let index = semantic_memory::HnswIndex::new(config).unwrap();

    let nan_query = vec![1.0, f32::NAN, 0.0, 0.5];
    let result = index.search(&nan_query, 5);
    assert!(result.is_err(), "NaN query vector must be rejected");
}
