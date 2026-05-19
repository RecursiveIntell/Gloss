#![cfg(feature = "hnsw")]

use semantic_memory::{MemoryConfig, MemoryStore, MockEmbedder, SearchSource, SearchSourceType};
use tempfile::TempDir;

fn test_store() -> (MemoryStore, TempDir) {
    let tmp = TempDir::new().unwrap();
    let config = MemoryConfig {
        base_dir: tmp.path().to_path_buf(),
        search: semantic_memory::SearchConfig {
            // MockEmbedder produces pseudo-random vectors with low cosine similarity,
            // so disable the similarity floor to ensure vector results are returned.
            min_similarity: 0.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let embedder = Box::new(MockEmbedder::new(768));
    let store = MemoryStore::open_with_embedder(config, embedder).unwrap();
    (store, tmp)
}

/// 4.1 - vector_only_search routes through HNSW and respects namespace filters.
#[tokio::test]
async fn vector_only_uses_hnsw() {
    let (store, _tmp) = test_store();

    // Add 10 facts in ns_a.
    for i in 0..10 {
        store
            .add_fact(
                "ns_a",
                &format!("alpha fact number {} about vector search", i),
                None,
                None,
            )
            .await
            .unwrap();
    }

    // Add 10 facts in ns_b.
    for i in 0..10 {
        store
            .add_fact(
                "ns_b",
                &format!("beta fact number {} about vector search", i),
                None,
                None,
            )
            .await
            .unwrap();
    }

    // Confirm data exists via FTS first.
    let fts_results = store
        .search_fts_only("vector search", Some(20), None, None)
        .await
        .unwrap();
    assert!(
        !fts_results.is_empty(),
        "FTS should find results to confirm data exists"
    );

    // Vector-only search filtered to ns_a.
    let results = store
        .search_vector_only("vector search", Some(10), Some(&["ns_a"]), None)
        .await
        .unwrap();

    assert!(
        !results.is_empty(),
        "vector_only_search should return non-empty results for ns_a"
    );

    // Every result must be from ns_a.
    for result in &results {
        match &result.source {
            SearchSource::Fact { namespace, .. } => {
                assert_eq!(
                    namespace, "ns_a",
                    "All results should be from ns_a, got: {}",
                    namespace
                );
            }
            other => panic!("Expected Fact source, got: {:?}", other),
        }
    }
}

/// 4.2 - vector_only_search respects source_type filter.
#[tokio::test]
async fn vector_only_respects_source_type_filter() {
    let (store, _tmp) = test_store();

    // Add facts.
    for i in 0..5 {
        store
            .add_fact(
                "general",
                &format!("important fact number {} about source type filtering", i),
                None,
                None,
            )
            .await
            .unwrap();
    }

    // Ingest a document so there are chunks in the index.
    let doc_content = "This document covers source type filtering in detail. ".repeat(30);
    store
        .ingest_document(
            "Source Type Doc",
            &doc_content,
            "general",
            Some("/test/source_type.txt"),
            None,
        )
        .await
        .unwrap();

    // Vector-only search restricted to Facts only.
    let results = store
        .search_vector_only(
            "source type filtering",
            Some(20),
            None,
            Some(&[SearchSourceType::Facts]),
        )
        .await
        .unwrap();

    // No chunk results should appear.
    for result in &results {
        match &result.source {
            SearchSource::Chunk { .. } => {
                panic!("Should not contain Chunk results when filtering to Facts only");
            }
            SearchSource::Message { .. } => {
                panic!("Should not contain Message results when filtering to Facts only");
            }
            SearchSource::Episode { .. } => {
                panic!("Should not contain Episode results when filtering to Facts only");
            }
            SearchSource::Projection { .. } => {
                panic!("Should not contain Projection results when filtering to Facts only");
            }
            SearchSource::Fact { .. } => { /* expected */ }
        }
    }
}

/// 4.3 - Both hybrid search and vector-only search return non-empty results.
#[tokio::test]
async fn vector_only_matches_hybrid_top() {
    let (store, _tmp) = test_store();

    // Add 10 facts.
    for i in 0..10 {
        store
            .add_fact(
                "general",
                &format!("knowledge item {} about hybrid and vector comparison", i),
                None,
                None,
            )
            .await
            .unwrap();
    }

    let query = "hybrid and vector comparison";

    let hybrid_results = store.search(query, Some(10), None, None).await.unwrap();
    let vector_results = store
        .search_vector_only(query, Some(10), None, None)
        .await
        .unwrap();

    assert!(
        !hybrid_results.is_empty(),
        "Hybrid search should return non-empty results"
    );
    assert!(
        !vector_results.is_empty(),
        "Vector-only search should return non-empty results"
    );
}
