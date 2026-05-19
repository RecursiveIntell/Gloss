#![allow(clippy::expect_used)]

use semantic_memory::search::{cosine_similarity, sanitize_fts_query, source_dedup_key};
#[cfg(feature = "turbo-quant-codec")]
use semantic_memory::DerivedVectorBackendPolicy;
use semantic_memory::SearchSource;
#[cfg(any(feature = "testing", feature = "turbo-quant-codec"))]
use semantic_memory::{ExactnessProfile, ReceiptMode, SearchContext};
use semantic_memory::{MemoryConfig, MemoryStore, MockEmbedder, SearchConfig, SearchSourceType};
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

#[cfg(feature = "turbo-quant-codec")]
fn turbo_quant_test_store() -> (MemoryStore, TempDir) {
    let tmp = TempDir::new().unwrap();
    let mut config = MemoryConfig {
        base_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    config.search.derived_vector_backend = DerivedVectorBackendPolicy::TurboQuantCandidateOnly;
    config.search.turbo_quant_bits = 8;
    config.search.turbo_quant_projections = 64;
    config.search.candidate_pool_size = 20;
    let embedder = Box::new(MockEmbedder::new(768));
    let store = MemoryStore::open_with_embedder(config, embedder).unwrap();
    (store, tmp)
}

// ─── Cosine Similarity ─────────────────────────────────────────

#[test]
fn cosine_identical_vectors() {
    let v = vec![1.0, 2.0, 3.0];
    let sim = cosine_similarity(&v, &v).unwrap();
    assert!(
        (sim - 1.0).abs() < 0.001,
        "Identical vectors should have similarity ~1.0, got {}",
        sim
    );
}

#[test]
fn cosine_orthogonal_vectors() {
    let a = vec![1.0, 0.0, 0.0];
    let b = vec![0.0, 1.0, 0.0];
    let sim = cosine_similarity(&a, &b).unwrap();
    assert!(
        sim.abs() < 0.001,
        "Orthogonal vectors should have similarity ~0.0, got {}",
        sim
    );
}

#[test]
fn cosine_opposite_vectors() {
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![-1.0, -2.0, -3.0];
    let sim = cosine_similarity(&a, &b).unwrap();
    assert!(
        (sim + 1.0).abs() < 0.001,
        "Opposite vectors should have similarity ~-1.0, got {}",
        sim
    );
}

#[test]
fn cosine_zero_vector() {
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![0.0, 0.0, 0.0];
    let sim = cosine_similarity(&a, &b).unwrap();
    assert_eq!(sim, 0.0, "Zero vector should return 0.0 similarity");
}

#[test]
fn cosine_mismatched_lengths_fail() {
    let err = cosine_similarity(&[1.0, 2.0], &[1.0]).unwrap_err();
    assert_eq!(err.kind(), "embedding_dimension_mismatch");
}

#[test]
fn cosine_non_finite_values_fail() {
    let err = cosine_similarity(&[1.0, f32::NAN], &[1.0, 2.0]).unwrap_err();
    assert_eq!(err.kind(), "non_finite_embedding_value");
}

// ─── FTS Query Sanitization ────────────────────────────────────

#[test]
fn sanitize_strips_fts_operators() {
    let result = sanitize_fts_query("hello \"world\" + test");
    assert_eq!(
        result,
        Some("\"hello\" OR \"world\" OR \"test\"".to_string())
    );
}

#[test]
fn sanitize_empty_after_stripping() {
    let result = sanitize_fts_query("\"*+-()^{}~:");
    assert_eq!(result, None);
}

#[test]
fn sanitize_normal_query_unchanged() {
    let result = sanitize_fts_query("hello world");
    assert_eq!(result, Some("\"hello\" OR \"world\"".to_string()));
}

#[test]
fn sanitize_unicode_preserved() {
    let result = sanitize_fts_query("中文 搜索");
    assert_eq!(result, Some("\"中文\" OR \"搜索\"".to_string()));
}

#[test]
fn sanitize_empty_string() {
    assert_eq!(sanitize_fts_query(""), None);
}

#[test]
fn sanitize_only_whitespace() {
    assert_eq!(sanitize_fts_query("   "), None);
}

// ─── FTS5 Punctuation Safety (question-mark bug) ─────────────

#[test]
fn sanitize_question_mark_in_chat() {
    // Root-cause regression: "how are you?" caused FTS5 syntax error
    let result = sanitize_fts_query("how are you?");
    assert_eq!(result, Some("\"how\" OR \"are\" OR \"you\"".to_string()));
}

#[test]
fn sanitize_question_mark_mid_sentence() {
    let result = sanitize_fts_query("what did i say about rust?");
    assert_eq!(
        result,
        Some("\"what\" OR \"did\" OR \"i\" OR \"say\" OR \"about\" OR \"rust\"".to_string())
    );
}

#[test]
fn sanitize_version_number_with_dot() {
    // "llama3.1" — dot is stripped, tokens merge or split
    let result = sanitize_fts_query("llama3.1");
    assert_eq!(result, Some("\"llama3\" OR \"1\"".to_string()));
}

#[test]
fn sanitize_quotes() {
    let result = sanitize_fts_query(r#"he said "hello" to me"#);
    assert_eq!(
        result,
        Some("\"he\" OR \"said\" OR \"hello\" OR \"to\" OR \"me\"".to_string())
    );
}

#[test]
fn sanitize_parentheses() {
    let result = sanitize_fts_query("function(arg1, arg2)");
    assert_eq!(
        result,
        Some("\"function\" OR \"arg1\" OR \"arg2\"".to_string())
    );
}

#[test]
fn sanitize_colons_and_dashes() {
    let result = sanitize_fts_query("key:value foo-bar");
    assert_eq!(
        result,
        Some("\"key\" OR \"value\" OR \"foo\" OR \"bar\"".to_string())
    );
}

#[test]
fn sanitize_slashes() {
    let result = sanitize_fts_query("path/to/file");
    assert_eq!(result, Some("\"path\" OR \"to\" OR \"file\"".to_string()));
}

#[test]
fn sanitize_mixed_punctuation() {
    let result = sanitize_fts_query("wait... what?! (really?)");
    assert_eq!(
        result,
        Some("\"wait\" OR \"what\" OR \"really\"".to_string())
    );
}

#[test]
fn sanitize_only_punctuation() {
    assert_eq!(sanitize_fts_query("?!@#$%^&*()"), None);
}

#[test]
fn sanitize_underscores_preserved() {
    let result = sanitize_fts_query("my_variable");
    assert_eq!(result, Some("\"my_variable\"".to_string()));
}

#[test]
fn message_dedup_key_includes_session_scope() {
    let a = SearchSource::Message {
        message_id: 7,
        session_id: "session-a".to_string(),
        role: "user".to_string(),
    };
    let b = SearchSource::Message {
        message_id: 7,
        session_id: "session-b".to_string(),
        role: "user".to_string(),
    };
    assert_ne!(source_dedup_key(&a), source_dedup_key(&b));
}

// ─── FTS5 Integration: Punctuated Queries Against Real DB ─────

#[tokio::test]
async fn fts_search_with_question_mark_does_not_crash() {
    let (store, _tmp) = test_store();
    store
        .add_fact("general", "I am doing well today", None, None)
        .await
        .unwrap();

    // This was the exact failure: "how are you?" → FTS5 syntax error
    let results = store
        .search_fts_only("how are you?", None, None, None)
        .await
        .unwrap();
    // May or may not match — the point is it must not error
    let _ = results;
}

#[tokio::test]
async fn fts_search_with_assorted_punctuation() {
    let (store, _tmp) = test_store();
    store
        .add_fact("general", "Rust is a systems language", None, None)
        .await
        .unwrap();

    let queries = vec![
        "what did i say about rust?",
        "llama3.1",
        r#"he said "hello""#,
        "function(arg)",
        "key:value",
        "path/to/file",
        "wait... what?! (really?)",
    ];
    for q in queries {
        let result = store.search_fts_only(q, None, None, None).await;
        assert!(
            result.is_ok(),
            "Query {:?} should not error: {:?}",
            q,
            result.err()
        );
    }
}

// ─── RRF Fusion ────────────────────────────────────────────────

#[test]
fn rrf_fusion_order() {
    // From SPEC.md §13:
    // BM25 results: [A(rank 1), B(rank 2), C(rank 3)]
    // Vector results: [B(rank 1), D(rank 2), A(rank 3)]
    // With k=60, weights=1.0:
    //   A: 1/61 + 1/63 = 0.01639 + 0.01587 = 0.03226
    //   B: 1/62 + 1/61 = 0.01613 + 0.01639 = 0.03252  <-- highest
    //   C: 1/63 + 0    = 0.01587
    //   D: 0    + 1/62 = 0.01613
    // Expected order: B, A, D, C

    use semantic_memory::search::{rrf_fuse, Bm25Hit, VectorHit};
    use semantic_memory::{SearchConfig, SearchSource};

    let make_fact_source = |id: &str| SearchSource::Fact {
        fact_id: id.to_string(),
        namespace: "test".to_string(),
    };

    let bm25_hits = vec![
        Bm25Hit {
            id: "A".to_string(),
            content: "content A".to_string(),
            source: make_fact_source("A"),
            raw_score: 0.1,
            updated_at: None,
        },
        Bm25Hit {
            id: "B".to_string(),
            content: "content B".to_string(),
            source: make_fact_source("B"),
            raw_score: 0.2,
            updated_at: None,
        },
        Bm25Hit {
            id: "C".to_string(),
            content: "content C".to_string(),
            source: make_fact_source("C"),
            raw_score: 0.3,
            updated_at: None,
        },
    ];

    let vector_hits = vec![
        VectorHit {
            id: "B".to_string(),
            content: "content B".to_string(),
            source: make_fact_source("B"),
            similarity: 0.9,
            updated_at: None,
            source_rank: Some(1),
            source_similarity: Some(0.9),
            reranked_from_f32: false,
        },
        VectorHit {
            id: "D".to_string(),
            content: "content D".to_string(),
            source: make_fact_source("D"),
            similarity: 0.8,
            updated_at: None,
            source_rank: Some(2),
            source_similarity: Some(0.8),
            reranked_from_f32: false,
        },
        VectorHit {
            id: "A".to_string(),
            content: "content A".to_string(),
            source: make_fact_source("A"),
            similarity: 0.7,
            updated_at: None,
            source_rank: Some(3),
            source_similarity: Some(0.7),
            reranked_from_f32: false,
        },
    ];

    let config = SearchConfig::default();
    let results = rrf_fuse(&bm25_hits, &vector_hits, &config, 10);

    assert_eq!(results.len(), 4);

    // Extract IDs in order
    let ids: Vec<String> = results
        .iter()
        .map(|r| match &r.source {
            SearchSource::Fact { fact_id, .. } => fact_id.clone(),
            SearchSource::Chunk { chunk_id, .. } => chunk_id.clone(),
            SearchSource::Message { message_id, .. } => message_id.to_string(),
            SearchSource::Episode { document_id, .. } => document_id.clone(),
            SearchSource::Projection { projection_id, .. } => projection_id.clone(),
        })
        .collect();

    assert_eq!(ids, vec!["B", "A", "D", "C"]);

    // Verify B has the highest score
    assert!(results[0].score > results[1].score);
}

// ─── Full Integration ──────────────────────────────────────────

#[tokio::test]
async fn hybrid_search_finds_facts() {
    let (store, _tmp) = test_store();

    store
        .add_fact(
            "general",
            "Rust is a systems programming language",
            None,
            None,
        )
        .await
        .unwrap();
    store
        .add_fact("general", "Python is great for data science", None, None)
        .await
        .unwrap();
    store
        .add_fact("general", "JavaScript runs in browsers", None, None)
        .await
        .unwrap();

    let results = store
        .search("systems programming", None, None, None)
        .await
        .unwrap();
    assert!(!results.is_empty(), "Hybrid search should return results");
}

#[tokio::test]
async fn fts_only_search() {
    let (store, _tmp) = test_store();

    store
        .add_fact(
            "general",
            "Rust is a systems programming language",
            None,
            None,
        )
        .await
        .unwrap();
    store
        .add_fact("general", "Python is great for data science", None, None)
        .await
        .unwrap();

    let results = store
        .search_fts_only("Rust systems", None, None, None)
        .await
        .unwrap();
    assert!(!results.is_empty());
    assert!(results[0].content.contains("Rust"));
}

#[tokio::test]
async fn search_with_namespace_filter() {
    let (store, _tmp) = test_store();

    store
        .add_fact("ns_a", "Fact in namespace A about dogs", None, None)
        .await
        .unwrap();
    store
        .add_fact("ns_b", "Fact in namespace B about dogs", None, None)
        .await
        .unwrap();

    let results = store
        .search_fts_only("dogs", None, Some(&["ns_a"]), None)
        .await
        .unwrap();
    assert_eq!(results.len(), 1, "Should only find fact in namespace A");
}

#[tokio::test]
async fn search_with_source_type_filter() {
    let (store, _tmp) = test_store();

    store
        .add_fact(
            "general",
            "This is a fact about quantum physics",
            None,
            None,
        )
        .await
        .unwrap();

    // Search only facts
    let results = store
        .search_fts_only(
            "quantum physics",
            None,
            None,
            Some(&[SearchSourceType::Facts]),
        )
        .await
        .unwrap();
    assert!(!results.is_empty());

    // Search only chunks (should be empty since we only have facts)
    let results = store
        .search_fts_only(
            "quantum physics",
            None,
            None,
            Some(&[SearchSourceType::Chunks]),
        )
        .await
        .unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn empty_query_returns_empty_results() {
    let (store, _tmp) = test_store();
    store
        .add_fact("general", "Some content", None, None)
        .await
        .unwrap();

    let results = store.search_fts_only("", None, None, None).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn special_chars_only_query_returns_empty() {
    let (store, _tmp) = test_store();
    store
        .add_fact("general", "Some content", None, None)
        .await
        .unwrap();

    let results = store
        .search_fts_only("\"*+-()^{}~:", None, None, None)
        .await
        .unwrap();
    assert!(results.is_empty());
}

// ─── Parameterized Namespace Filtering (Fix 1) ───────────────

#[tokio::test]
async fn parameterized_namespace_adversarial() {
    let (store, _tmp) = test_store();

    store
        .add_fact("safe", "Safe fact about cats", None, None)
        .await
        .unwrap();
    store
        .add_fact("also-safe", "Also safe fact about cats", None, None)
        .await
        .unwrap();
    // Adversarial namespace with a single quote
    store
        .add_fact(
            "it's-a-test",
            "Adversarial namespace fact about cats",
            None,
            None,
        )
        .await
        .unwrap();

    // Search with adversarial namespace — should find it, not crash
    let results = store
        .search_fts_only("cats", None, Some(&["it's-a-test"]), None)
        .await
        .unwrap();
    assert_eq!(
        results.len(),
        1,
        "Should find fact in adversarial namespace"
    );
    assert!(results[0].content.contains("Adversarial"));

    // Search with safe namespace — should only find safe fact
    let results = store
        .search_fts_only("cats", None, Some(&["safe"]), None)
        .await
        .unwrap();
    assert_eq!(
        results.len(),
        1,
        "Should only find fact in 'safe' namespace"
    );
    assert!(results[0].content.contains("Safe fact"));
}

// ─── Content Deduplication (Fix 6) ───────────────────────────

#[tokio::test]
async fn dedup_removes_duplicate_content() {
    let (store, _tmp) = test_store();

    // Add a fact with specific content
    store
        .add_fact("general", "Rust was released in 2015", None, None)
        .await
        .unwrap();

    // Ingest a document with a chunk containing the exact same text
    store
        .ingest_document(
            "Rust History",
            "Rust was released in 2015",
            "general",
            None,
            None,
        )
        .await
        .unwrap();

    // After provenance-based dedup, both the fact and chunk are kept
    // because they come from different source types (Fact vs Chunk).
    let results = store
        .search("Rust released", None, None, None)
        .await
        .unwrap();
    assert_eq!(
        results.len(),
        2,
        "Should keep results from different source types even with identical content"
    );
}

#[tokio::test]
async fn dedup_keeps_different_content() {
    let (store, _tmp) = test_store();

    store
        .add_fact(
            "general",
            "Rust was released as a language in 2015",
            None,
            None,
        )
        .await
        .unwrap();
    store
        .add_fact(
            "general",
            "Go was released as a language in 2009",
            None,
            None,
        )
        .await
        .unwrap();

    // FTS will find both since they share the word "released" and "language"
    let results = store
        .search_fts_only("released language", None, None, None)
        .await
        .unwrap();
    assert_eq!(
        results.len(),
        2,
        "Should keep both results since content is different"
    );
}

// ─── Recency Weighting (Fix 3) ──────────────────────────────

fn test_store_with_recency(half_life: Option<f64>, recency_weight: f64) -> (MemoryStore, TempDir) {
    let tmp = TempDir::new().unwrap();
    let config = MemoryConfig {
        base_dir: tmp.path().to_path_buf(),
        search: SearchConfig {
            recency_half_life_days: half_life,
            recency_weight,
            ..Default::default()
        },
        ..Default::default()
    };
    let embedder = Box::new(MockEmbedder::new(768));
    let store = MemoryStore::open_with_embedder(config, embedder).unwrap();
    (store, tmp)
}

#[tokio::test]
async fn recency_disabled_no_effect() {
    // recency_half_life_days: None → same behavior as V1
    let (store, _tmp) = test_store_with_recency(None, 0.5);

    store
        .add_fact("general", "Recency test fact alpha", None, None)
        .await
        .unwrap();

    let results = store
        .search_fts_only("Recency test fact", None, None, None)
        .await
        .unwrap();
    assert!(!results.is_empty());
    // Score should be purely BM25-based with no recency component
    let expected_score = 1.0 / (60.0 + 1.0); // bm25_weight / (rrf_k + rank)
    assert!(
        (results[0].score - expected_score).abs() < 0.0001,
        "Score should be pure BM25 RRF score without recency, got {} expected {}",
        results[0].score,
        expected_score
    );
}

#[cfg(feature = "testing")]
#[tokio::test]
async fn recency_boosts_recent_facts() {
    let (store, _tmp) = test_store_with_recency(Some(30.0), 0.5);

    // Add two facts with the same content relevance
    let fact_a_id = store
        .add_fact(
            "general",
            "Recency quantum computing breakthrough",
            None,
            None,
        )
        .await
        .unwrap();
    let fact_b_id = store
        .add_fact("general", "Recency quantum computing discovery", None, None)
        .await
        .unwrap();

    // Set fact B to 60 days ago
    let sixty_days_ago = (chrono::Utc::now() - chrono::Duration::days(60))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    store
        .raw_execute(
            "UPDATE facts SET updated_at = ?1 WHERE id = ?2",
            vec![sixty_days_ago, fact_b_id.clone()],
        )
        .await
        .unwrap();

    let results = store
        .search("quantum computing", None, None, None)
        .await
        .unwrap();
    assert!(results.len() >= 2, "Should find both facts");

    // Find scores for each fact
    let score_a = results
        .iter()
        .find(|r| match &r.source {
            SearchSource::Fact { fact_id, .. } => fact_id == &fact_a_id,
            _ => false,
        })
        .map(|r| r.score);
    let score_b = results
        .iter()
        .find(|r| match &r.source {
            SearchSource::Fact { fact_id, .. } => fact_id == &fact_b_id,
            _ => false,
        })
        .map(|r| r.score);

    assert!(
        score_a.unwrap() > score_b.unwrap(),
        "Recent fact A ({}) should score higher than old fact B ({})",
        score_a.unwrap(),
        score_b.unwrap()
    );
}

#[cfg(feature = "testing")]
#[tokio::test]
async fn recency_is_deterministic_for_same_search_context_time() {
    let (store, _tmp) = test_store_with_recency(Some(30.0), 0.5);
    let fact_id = store
        .add_fact("general", "Deterministic recency fixture", None, None)
        .await
        .unwrap();
    store
        .raw_execute(
            "UPDATE facts SET updated_at = ?1 WHERE id = ?2",
            vec!["2026-01-01 00:00:00".to_string(), fact_id.clone()],
        )
        .await
        .unwrap();

    let mut context = SearchContext::at(
        chrono::DateTime::parse_from_rfc3339("2026-02-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    );
    context.exactness_profile = ExactnessProfile::PreferExact;

    let first = store
        .search_explained_with_context(
            "Deterministic recency",
            Some(5),
            None,
            Some(&[SearchSourceType::Facts]),
            context.clone(),
        )
        .await
        .unwrap();
    let second = store
        .search_explained_with_context(
            "Deterministic recency",
            Some(5),
            None,
            Some(&[SearchSourceType::Facts]),
            context,
        )
        .await
        .unwrap();

    let first_score = first
        .results
        .iter()
        .find(|result| matches!(&result.result.source, SearchSource::Fact { fact_id: id, .. } if id == &fact_id))
        .unwrap()
        .breakdown
        .recency_score;
    let second_score = second
        .results
        .iter()
        .find(|result| matches!(&result.result.source, SearchSource::Fact { fact_id: id, .. } if id == &fact_id))
        .unwrap()
        .breakdown
        .recency_score;
    assert_eq!(first_score, second_score);
}

#[cfg(feature = "testing")]
#[tokio::test]
async fn recency_changes_with_different_search_context_times() {
    let (store, _tmp) = test_store_with_recency(Some(30.0), 0.5);
    let fact_id = store
        .add_fact("general", "Replay recency fixture", None, None)
        .await
        .unwrap();
    store
        .raw_execute(
            "UPDATE facts SET updated_at = ?1 WHERE id = ?2",
            vec!["2026-01-01 00:00:00".to_string(), fact_id.clone()],
        )
        .await
        .unwrap();

    let mut early = SearchContext::at(
        chrono::DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    );
    early.exactness_profile = ExactnessProfile::PreferExact;
    let mut late = SearchContext::at(
        chrono::DateTime::parse_from_rfc3339("2026-03-02T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    );
    late.exactness_profile = ExactnessProfile::PreferExact;

    let early_results = store
        .search_explained_with_context(
            "Replay recency",
            Some(5),
            None,
            Some(&[SearchSourceType::Facts]),
            early,
        )
        .await
        .unwrap();
    let late_results = store
        .search_explained_with_context(
            "Replay recency",
            Some(5),
            None,
            Some(&[SearchSourceType::Facts]),
            late,
        )
        .await
        .unwrap();

    let early_recency = early_results
        .results
        .iter()
        .find(|result| matches!(&result.result.source, SearchSource::Fact { fact_id: id, .. } if id == &fact_id))
        .unwrap()
        .breakdown
        .recency_score
        .unwrap();
    let late_recency = late_results
        .results
        .iter()
        .find(|result| matches!(&result.result.source, SearchSource::Fact { fact_id: id, .. } if id == &fact_id))
        .unwrap()
        .breakdown
        .recency_score
        .unwrap();
    assert!(
        early_recency > late_recency,
        "recency should decay as evaluation_time moves forward"
    );
}

#[cfg(feature = "testing")]
#[tokio::test]
async fn context_search_receipt_records_exact_backend_and_result_ids() {
    let (store, _tmp) = test_store_with_recency(None, 0.0);
    let fact_id = store
        .add_fact("general", "Receipt exact backend fixture", None, None)
        .await
        .unwrap();
    let mut context = SearchContext::at(
        chrono::DateTime::parse_from_rfc3339("2026-02-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    );
    context.receipt_mode = ReceiptMode::ReturnReceipt;
    context.exactness_profile = ExactnessProfile::PreferExact;
    context.request_id = Some("receipt-test".to_string());

    let response = store
        .search_with_context(
            "Receipt exact backend",
            Some(3),
            None,
            Some(&[SearchSourceType::Facts]),
            context,
        )
        .await
        .unwrap();
    let receipt = response.receipt.expect("receipt should be returned");
    assert_eq!(receipt.receipt_id, "receipt-test");
    assert_eq!(receipt.candidate_backend, "brute_force_f32");
    assert_eq!(receipt.fallback, None);
    assert!(receipt.exact_rerank);
    assert!(receipt.result_ids.contains(&format!("fact:{fact_id}")));

    let answers = receipt.answers();
    assert_eq!(answers.replay_receipt_id, "receipt-test");
    assert_eq!(answers.exactness, "exact_reference_with_rerank");
    assert!(answers.replay_ready);
    assert!(answers.rebuild_ready);
    assert!(!answers.approximate);
    assert_eq!(answers.result_count, answers.result_ids.len());
    assert!(answers
        .why_results_appeared
        .iter()
        .any(|reason| reason.contains("brute_force_f32")));

    let durable = store.get_search_receipt("receipt-test").await.unwrap();
    let durable = match durable {
        Some(receipt) => receipt,
        None => panic!("receipt should be durable"),
    };
    assert_eq!(durable.receipt_id, receipt.receipt_id);
    assert_eq!(durable.candidate_backend, receipt.candidate_backend);
    assert_eq!(durable.result_ids, receipt.result_ids);
}

#[cfg(feature = "turbo-quant-codec")]
mod search_tests {
    mod turbo_quant {
        use super::super::*;

        #[tokio::test]
        async fn candidate_path_sets_receipt_profile() {
            let (store, _tmp) = turbo_quant_test_store();
            let fact_id = store
                .add_fact(
                    "general",
                    "TurboQuant receipt candidate fixture",
                    None,
                    None,
                )
                .await
                .unwrap();
            let build = store.rebuild_vector_artifacts().await.unwrap();
            assert_eq!(build.codec_family, "turbo_quant");
            assert_eq!(build.source_row_count, 1);
            assert_eq!(build.artifact_count, 1);

            let mut context = SearchContext::default_now();
            context.receipt_mode = ReceiptMode::ReturnReceipt;
            let response = store
                .search_vector_only_with_context(
                    "TurboQuant receipt candidate fixture",
                    Some(1),
                    None,
                    None,
                    context,
                )
                .await
                .unwrap();
            let receipt = match response.receipt {
                Some(receipt) => receipt,
                None => panic!("receipt should be returned"),
            };
            assert_eq!(
                receipt.candidate_backend,
                "turbo_quant_candidate_then_exact_f32"
            );
            assert_eq!(receipt.codec_family.as_deref(), Some("turbo_quant"));
            assert_eq!(
                receipt.codec_profile_digest.as_deref(),
                Some(build.codec_profile_digest.as_str())
            );
            assert_eq!(receipt.artifact_count, Some(1));
            assert_eq!(receipt.artifact_corruption_count, Some(0));
            assert_eq!(receipt.artifact_missing_count, Some(0));
            assert_eq!(receipt.vector_artifact_count, Some(1));
            assert_eq!(receipt.vector_artifact_missing_count, Some(0));
            assert_eq!(receipt.vector_artifact_stale_count, Some(0));
            assert_eq!(receipt.artifact_generation_id, build.generation_id);
            assert!(receipt
                .vector_artifact_manifest_digest
                .as_deref()
                .is_some_and(|digest| digest.starts_with("blake3:")));
            assert_eq!(receipt.approximate_candidate_count, Some(1));
            assert_eq!(receipt.approximate_scanned_count, Some(1));
            assert_eq!(receipt.approximate_returned_count, Some(1));
            assert_eq!(receipt.exact_rerank_count, Some(1));
            assert_eq!(receipt.raw_rows_loaded_count, Some(1));
            assert_eq!(
                receipt.filter_strategy.as_deref(),
                Some("unfiltered_top_k_heap")
            );
            assert!(receipt.approximate);
            assert!(receipt.exact_rerank);
            assert!(receipt.result_ids.contains(&format!("fact:{fact_id}")));
        }

        #[tokio::test]
        async fn missing_artifacts_fallback_to_brute_force() {
            let (store, _tmp) = turbo_quant_test_store();
            let fact_id = store
                .add_fact(
                    "general",
                    "TurboQuant missing artifact fallback fixture",
                    None,
                    None,
                )
                .await
                .unwrap();

            let mut context = SearchContext::default_now();
            context.receipt_mode = ReceiptMode::ReturnReceipt;
            let response = store
                .search_vector_only_with_context(
                    "TurboQuant missing artifact fallback fixture",
                    Some(1),
                    None,
                    None,
                    context,
                )
                .await
                .unwrap();
            let receipt = match response.receipt {
                Some(receipt) => receipt,
                None => panic!("receipt should be returned"),
            };
            assert_eq!(
                receipt.fallback.as_deref(),
                Some("turbo_quant_generation_missing_or_invalidated")
            );
            assert_eq!(receipt.artifact_missing_count, Some(1));
            assert_eq!(receipt.vector_artifact_missing_count, Some(1));
            assert_eq!(
                receipt.fallback_reason.as_deref(),
                Some("turbo_quant_generation_missing_or_invalidated")
            );
            assert!(receipt.result_ids.contains(&format!("fact:{fact_id}")));
        }

        #[tokio::test]
        async fn prefer_exact_bypasses_candidate_path() {
            let (store, _tmp) = turbo_quant_test_store();
            store
                .add_fact("general", "TurboQuant prefer exact fixture", None, None)
                .await
                .unwrap();
            store.rebuild_vector_artifacts().await.unwrap();

            let mut context = SearchContext::default_now();
            context.receipt_mode = ReceiptMode::ReturnReceipt;
            context.exactness_profile = ExactnessProfile::PreferExact;
            let response = store
                .search_vector_only_with_context(
                    "TurboQuant prefer exact fixture",
                    Some(1),
                    None,
                    None,
                    context,
                )
                .await
                .unwrap();
            let receipt = match response.receipt {
                Some(receipt) => receipt,
                None => panic!("receipt should be returned"),
            };
            assert_eq!(receipt.candidate_backend, "brute_force_f32");
            assert_eq!(receipt.codec_family, None);
            assert!(!receipt.approximate);
        }

        #[tokio::test]
        async fn derived_vector_artifact_rebuild_is_idempotent() {
            let (store, _tmp) = turbo_quant_test_store();
            store
                .add_fact(
                    "general",
                    "TurboQuant rebuild idempotent fixture",
                    None,
                    None,
                )
                .await
                .unwrap();
            let first = store.rebuild_vector_artifacts().await.unwrap();
            let second = store.rebuild_vector_artifacts().await.unwrap();

            assert_eq!(first.codec_family, "turbo_quant");
            assert_eq!(first.codec_profile_digest, second.codec_profile_digest);
            assert_eq!(first.source_row_count, 1);
            assert_eq!(second.source_row_count, 1);
            assert_eq!(first.artifact_count, 1);
            assert_eq!(second.artifact_count, 1);
            assert_eq!(second.skipped_row_count, 0);
        }

        #[cfg(feature = "testing")]
        #[tokio::test]
        async fn stale_generation_snapshot_falls_back_to_brute_force() {
            let (store, _tmp) = turbo_quant_test_store();
            let fact_id = store
                .add_fact(
                    "general",
                    "TurboQuant stale generation snapshot fallback fixture",
                    None,
                    None,
                )
                .await
                .unwrap();
            store.rebuild_vector_artifacts().await.unwrap();
            store
                .raw_execute(
                    "UPDATE derived_vector_artifact_generations
                     SET source_snapshot_digest = 'blake3:stale'
                     WHERE status = 'active'",
                    vec![],
                )
                .await
                .unwrap();

            let mut context = SearchContext::default_now();
            context.receipt_mode = ReceiptMode::ReturnReceipt;
            let response = store
                .search_vector_only_with_context(
                    "TurboQuant stale generation snapshot fallback fixture",
                    Some(1),
                    None,
                    None,
                    context,
                )
                .await
                .unwrap();
            let receipt = response.receipt.expect("receipt should be returned");
            assert_eq!(
                receipt.fallback.as_deref(),
                Some("turbo_quant_generation_incomplete_or_stale")
            );
            assert!(receipt
                .degradations
                .iter()
                .any(|note| note.contains("generation validation failed")));
            assert!(receipt.result_ids.contains(&format!("fact:{fact_id}")));
        }

        #[cfg(feature = "testing")]
        #[tokio::test]
        async fn corrupt_artifact_fallback_records_degradation() {
            let (store, _tmp) = turbo_quant_test_store();
            let fact_id = store
                .add_fact(
                    "general",
                    "TurboQuant corrupt artifact fallback fixture",
                    None,
                    None,
                )
                .await
                .unwrap();
            store.rebuild_vector_artifacts().await.unwrap();
            store
                .raw_execute(
                    "UPDATE derived_vector_artifacts SET encoded = x'00'",
                    vec![],
                )
                .await
                .unwrap();

            let mut context = SearchContext::default_now();
            context.receipt_mode = ReceiptMode::ReturnReceipt;
            let response = store
                .search_vector_only_with_context(
                    "TurboQuant corrupt artifact fallback fixture",
                    Some(1),
                    None,
                    None,
                    context,
                )
                .await
                .unwrap();
            let receipt = match response.receipt {
                Some(receipt) => receipt,
                None => panic!("receipt should be returned"),
            };
            assert_eq!(
                receipt.fallback.as_deref(),
                Some("turbo_quant_artifact_validation_failed")
            );
            assert_eq!(receipt.artifact_corruption_count, Some(1));
            assert!(receipt
                .degradations
                .iter()
                .any(|note| note.contains("artifact validation failed")));
            assert!(receipt.result_ids.contains(&format!("fact:{fact_id}")));
        }
    }
}

#[cfg(feature = "testing")]
#[tokio::test]
async fn explained_result_answer_names_source_and_score_lanes() {
    let (store, _tmp) = test_store_with_recency(None, 0.0);
    let fact_id = store
        .add_fact("general", "Why this result receipt fixture", None, None)
        .await
        .unwrap();
    let mut context = SearchContext::at(
        chrono::DateTime::parse_from_rfc3339("2026-02-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    );
    context.receipt_mode = ReceiptMode::ExplainOnly;
    context.exactness_profile = ExactnessProfile::PreferExact;

    let response = store
        .search_explained_with_context(
            "Why this result",
            Some(3),
            None,
            Some(&[SearchSourceType::Facts]),
            context,
        )
        .await
        .unwrap();
    let explained = response
        .results
        .iter()
        .find(|result| matches!(&result.result.source, SearchSource::Fact { fact_id: id, .. } if id == &fact_id))
        .expect("inserted fact should be returned");
    let answer = explained.answer();
    assert_eq!(answer.result_id, format!("fact:{fact_id}"));
    assert_eq!(answer.source_kind, "fact");
    assert_eq!(answer.source_id, fact_id);
    assert!(answer.text_match || answer.vector_match);
    assert!(!answer.why_this_result.is_empty());
}

#[cfg(feature = "testing")]
#[tokio::test]
async fn durable_receipt_id_conflict_fails_closed() {
    let (store, _tmp) = test_store_with_recency(None, 0.0);
    store
        .add_fact("general", "Durable receipt alpha", None, None)
        .await
        .unwrap();
    store
        .add_fact("general", "Durable receipt beta", None, None)
        .await
        .unwrap();

    let mut context = SearchContext::at(
        chrono::DateTime::parse_from_rfc3339("2026-02-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    );
    context.receipt_mode = ReceiptMode::ReturnReceipt;
    context.exactness_profile = ExactnessProfile::PreferExact;
    context.request_id = Some("receipt-collision".to_string());

    store
        .search_with_context(
            "Durable receipt alpha",
            Some(3),
            None,
            Some(&[SearchSourceType::Facts]),
            context.clone(),
        )
        .await
        .unwrap();

    let err = store
        .search_with_context(
            "Durable receipt beta",
            Some(3),
            None,
            Some(&[SearchSourceType::Facts]),
            context,
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "search_receipt_conflict");
}

#[cfg(feature = "testing")]
#[tokio::test]
async fn durable_receipt_replay_matches_original_inputs() {
    let (store, _tmp) = test_store_with_recency(None, 0.0);
    let fact_id = store
        .add_fact("general", "Replayable durable receipt fixture", None, None)
        .await
        .unwrap();

    let mut context = SearchContext::at(
        chrono::DateTime::parse_from_rfc3339("2026-02-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    );
    context.receipt_mode = ReceiptMode::ReturnReceipt;
    context.exactness_profile = ExactnessProfile::PreferExact;
    context.request_id = Some("receipt-replay-match".to_string());

    store
        .search_with_context(
            "Replayable durable receipt",
            Some(1),
            None,
            Some(&[SearchSourceType::Facts]),
            context,
        )
        .await
        .unwrap();

    let report = store
        .replay_search_receipt(
            "receipt-replay-match",
            "Replayable durable receipt",
            Some(1),
            None,
            Some(&[SearchSourceType::Facts]),
        )
        .await
        .unwrap();

    assert!(report.query_embedding_digest_matches);
    assert!(report.result_ids_match);
    assert!(report.missing_result_ids.is_empty());
    assert!(report.added_result_ids.is_empty());
    assert_eq!(report.receipt_id, "receipt-replay-match");
    assert!(report
        .replay_receipt
        .result_ids
        .contains(&format!("fact:{fact_id}")));

    let durable_replay = store
        .get_search_receipt(&report.replay_receipt_id)
        .await
        .unwrap();
    assert!(
        durable_replay.is_some(),
        "replay attempt should leave its own receipt"
    );
}

#[cfg(feature = "testing")]
#[tokio::test]
async fn durable_receipt_replay_detects_wrong_query() {
    let (store, _tmp) = test_store_with_recency(None, 0.0);
    store
        .add_fact("general", "Replay alpha source text", None, None)
        .await
        .unwrap();
    store
        .add_fact("general", "Replay beta source text", None, None)
        .await
        .unwrap();

    let mut context = SearchContext::at(
        chrono::DateTime::parse_from_rfc3339("2026-02-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    );
    context.receipt_mode = ReceiptMode::ReturnReceipt;
    context.exactness_profile = ExactnessProfile::PreferExact;
    context.request_id = Some("receipt-replay-drift".to_string());

    store
        .search_with_context(
            "Replay alpha",
            Some(1),
            None,
            Some(&[SearchSourceType::Facts]),
            context,
        )
        .await
        .unwrap();

    let report = store
        .replay_search_receipt(
            "receipt-replay-drift",
            "Replay beta",
            Some(1),
            None,
            Some(&[SearchSourceType::Facts]),
        )
        .await
        .unwrap();

    assert!(!report.query_embedding_digest_matches);
    assert!(!report.result_ids_match);
    assert!(!report.missing_result_ids.is_empty() || !report.added_result_ids.is_empty());
}

#[cfg(feature = "testing")]
#[tokio::test]
async fn replay_missing_receipt_id_fails_closed() {
    let (store, _tmp) = test_store_with_recency(None, 0.0);
    let err = store
        .replay_search_receipt(
            "missing-receipt",
            "anything",
            Some(1),
            None,
            Some(&[SearchSourceType::Facts]),
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "search_receipt_not_found");
}

#[cfg(feature = "testing")]
#[tokio::test]
async fn unsupported_durable_receipt_schema_version_fails_closed() {
    let (store, _tmp) = test_store_with_recency(None, 0.0);
    store
        .add_fact("general", "Receipt schema fixture", None, None)
        .await
        .unwrap();
    let mut context = SearchContext::at(
        chrono::DateTime::parse_from_rfc3339("2026-02-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    );
    context.receipt_mode = ReceiptMode::ReturnReceipt;
    context.exactness_profile = ExactnessProfile::PreferExact;
    context.request_id = Some("receipt-future-schema".to_string());

    store
        .search_with_context(
            "Receipt schema",
            Some(3),
            None,
            Some(&[SearchSourceType::Facts]),
            context,
        )
        .await
        .unwrap();
    store
        .raw_execute(
            "UPDATE search_receipts SET schema_version = ?1 WHERE receipt_id = ?2",
            vec![
                "vector_search_receipt_v99".to_string(),
                "receipt-future-schema".to_string(),
            ],
        )
        .await
        .unwrap();

    let err = store
        .get_search_receipt("receipt-future-schema")
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "corrupt_data");
}

#[cfg(all(feature = "hnsw", feature = "testing"))]
#[tokio::test]
async fn filtered_hnsw_underreturn_records_exact_fallback_receipt() {
    let (store, _tmp) = test_store_with_recency(None, 0.0);
    for idx in 0..6 {
        store
            .add_fact(
                "other",
                &format!("HNSW fallback other fact {idx}"),
                None,
                None,
            )
            .await
            .unwrap();
    }
    store
        .add_fact("target", "HNSW fallback target fact", None, None)
        .await
        .unwrap();

    let mut context = SearchContext::at(
        chrono::DateTime::parse_from_rfc3339("2026-02-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    );
    context.receipt_mode = ReceiptMode::ReturnReceipt;
    context.exactness_profile = ExactnessProfile::AllowApproximate;

    let response = store
        .search_with_context(
            "HNSW fallback fact",
            Some(2),
            Some(&["target"]),
            Some(&[SearchSourceType::Facts]),
            context,
        )
        .await
        .unwrap();
    let receipt = response.receipt.expect("receipt should be returned");
    assert_eq!(
        receipt.fallback.as_deref(),
        Some("hnsw_filtered_underreturn_fallback")
    );
    assert_eq!(receipt.candidate_backend, "hnsw_then_brute_force_f32");
    assert!(receipt.exact_rerank);
    assert!(!receipt.degradations.is_empty());

    let answers = receipt.answers();
    assert_eq!(
        answers.exactness,
        "approximate_candidate_generation_with_exact_rerank"
    );
    assert!(answers.approximate);
    assert!(answers.degraded);
    assert_eq!(
        answers.fallback.as_deref(),
        Some("hnsw_filtered_underreturn_fallback")
    );
}

#[tokio::test]
async fn recency_zero_half_life_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let config = MemoryConfig {
        base_dir: tmp.path().to_path_buf(),
        search: SearchConfig {
            recency_half_life_days: Some(0.0),
            recency_weight: 0.5,
            ..Default::default()
        },
        ..Default::default()
    };
    let embedder = Box::new(MockEmbedder::new(768));
    let err = match MemoryStore::open_with_embedder(config, embedder) {
        Ok(_) => panic!("zero recency half-life should be rejected"),
        Err(err) => err,
    };
    assert_eq!(err.kind(), "invalid_config");
}

#[tokio::test]
async fn invalid_ollama_url_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let config = MemoryConfig {
        base_dir: tmp.path().to_path_buf(),
        embedding: semantic_memory::EmbeddingConfig {
            ollama_url: "not a url".to_string(),
            ..Default::default()
        },
        ..Default::default()
    };
    let embedder = Box::new(MockEmbedder::new(768));
    let err = match MemoryStore::open_with_embedder(config, embedder) {
        Ok(_) => panic!("invalid Ollama URL should be rejected"),
        Err(err) => err,
    };
    assert_eq!(err.kind(), "invalid_config");
}

#[cfg(feature = "turbo-quant-codec")]
#[tokio::test]
async fn turbo_quant_candidate_backend_requires_safe_bits_and_exact_rerank() {
    use semantic_memory::DerivedVectorBackendPolicy;

    let tmp = TempDir::new().unwrap();
    let config = MemoryConfig {
        base_dir: tmp.path().to_path_buf(),
        search: SearchConfig {
            derived_vector_backend: DerivedVectorBackendPolicy::TurboQuantCandidateOnly,
            turbo_quant_bits: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    let err = match MemoryStore::open_with_embedder(config, Box::new(MockEmbedder::new(768))) {
        Ok(_) => panic!("TurboQuant bits=1 should be rejected"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("2..=16"));

    let tmp = TempDir::new().unwrap();
    let config = MemoryConfig {
        base_dir: tmp.path().to_path_buf(),
        search: SearchConfig {
            derived_vector_backend: DerivedVectorBackendPolicy::TurboQuantCandidateOnly,
            turbo_quant_require_exact_rerank: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let err = match MemoryStore::open_with_embedder(config, Box::new(MockEmbedder::new(768))) {
        Ok(_) => panic!("TurboQuant without exact rerank should be rejected"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("requires exact f32 rerank"));
}

// ─── V2: Buffer Reuse Correctness (Fix 6 regression) ────────

#[tokio::test]
async fn test_vector_search_buffer_reuse_correctness() {
    let (store, _tmp) = test_store();

    // Insert 100 facts with known embeddings
    for i in 0..100 {
        store
            .add_fact(
                "general",
                &format!("Buffer reuse test fact number {}", i),
                None,
                None,
            )
            .await
            .unwrap();
    }

    // Search and verify results are returned correctly
    let results = store
        .search("Buffer reuse test fact", None, None, None)
        .await
        .unwrap();
    assert!(!results.is_empty(), "Should find facts with buffer reuse");

    // Verify scores are valid (not NaN, not infinite)
    for result in &results {
        assert!(
            result.score.is_finite(),
            "Score should be finite, got {}",
            result.score
        );
        assert!(
            result.score >= 0.0,
            "Score should be non-negative, got {}",
            result.score
        );
    }

    // Verify results are ordered by score descending
    for i in 1..results.len() {
        assert!(
            results[i - 1].score >= results[i].score,
            "Results should be ordered by score descending: {} < {}",
            results[i - 1].score,
            results[i].score
        );
    }
}

// ─── V2: Large Row Count Warning (Fix 9) ────────────────────

#[tokio::test]
async fn test_vector_search_completes_with_many_rows() {
    let (store, _tmp) = test_store();

    // Insert 100 facts (can't easily test 50K in unit tests)
    for i in 0..100 {
        store
            .add_fact(
                "general",
                &format!("Row count test fact number {}", i),
                None,
                None,
            )
            .await
            .unwrap();
    }

    // Search should succeed — the warning threshold is about logging, not blocking
    let results = store
        .search("Row count test fact", None, None, None)
        .await
        .unwrap();
    assert!(
        !results.is_empty(),
        "Search should complete successfully with many rows"
    );
}
