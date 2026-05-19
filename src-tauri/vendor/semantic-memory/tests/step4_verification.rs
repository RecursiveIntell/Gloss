//! Step 4 verification tests: Episodes, explainable search, embedding displacement.
#![allow(clippy::expect_used)]

use semantic_memory::{
    EmbeddingDisplacement, EpisodeMeta, EpisodeOutcome, ExplainedResult, MemoryConfig, MemoryStore,
    MockEmbedder, ScoreBreakdown, VerificationStatus,
};
use tempfile::TempDir;

fn test_config(dir: &TempDir) -> MemoryConfig {
    MemoryConfig {
        base_dir: dir.path().to_path_buf(),
        ..Default::default()
    }
}

fn open_store(dir: &TempDir) -> MemoryStore {
    let config = test_config(dir);
    let embedder = Box::new(MockEmbedder::new(config.embedding.dimensions));
    MemoryStore::open_with_embedder(config, embedder).expect("open store")
}

// ─── Episode Lifecycle Tests ──────────────────────────────────

#[tokio::test]
async fn test_episode_ingest_and_search() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    // Create a document first (episodes reference documents)
    store
        .ingest_document(
            "Episode Test Doc",
            "This document describes a test failure in the auth module.",
            "tests",
            None,
            None,
        )
        .await
        .unwrap();

    // Get the document ID
    let docs = store.list_documents("tests", 10, 0).await.unwrap();
    assert!(!docs.is_empty());
    let doc_id = &docs[0].id;

    // Ingest an episode
    let meta = EpisodeMeta {
        cause_ids: vec!["fact-1".to_string(), "fact-2".to_string()],
        effect_type: "test_failure".to_string(),
        outcome: EpisodeOutcome::Pending,
        confidence: 0.5,
        verification_status: VerificationStatus::Unverified,
        experiment_id: None,
    };
    store.ingest_episode(doc_id, &meta).await.unwrap();

    // Search episodes by effect type
    let results = store
        .search_episodes(Some("test_failure"), None, 10)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, *doc_id);
    assert_eq!(results[0].1.effect_type, "test_failure");
    assert_eq!(results[0].1.outcome, EpisodeOutcome::Pending);
}

#[tokio::test]
async fn test_episode_update_outcome() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    store
        .ingest_document("Doc for episode", "Content here", "ns", None, None)
        .await
        .unwrap();
    let docs = store.list_documents("ns", 10, 0).await.unwrap();
    let doc_id = &docs[0].id;

    let meta = EpisodeMeta {
        cause_ids: vec!["cause-1".to_string()],
        effect_type: "regression".to_string(),
        outcome: EpisodeOutcome::Pending,
        confidence: 0.3,
        verification_status: VerificationStatus::Unverified,
        experiment_id: None,
    };
    store.ingest_episode(doc_id, &meta).await.unwrap();

    // Update outcome
    store
        .update_episode_outcome(doc_id, EpisodeOutcome::Confirmed, 0.9, Some("exp-123"))
        .await
        .unwrap();

    // Verify update
    let results = store.search_episodes(None, None, 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].1.outcome, EpisodeOutcome::Confirmed);
    assert!((results[0].1.confidence - 0.9).abs() < 0.01);
    assert_eq!(results[0].1.experiment_id.as_deref(), Some("exp-123"));
}

#[tokio::test]
async fn test_episode_search_by_outcome() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    // Create two documents
    store
        .ingest_document("Doc A", "Content A", "ns", None, None)
        .await
        .unwrap();
    store
        .ingest_document("Doc B", "Content B", "ns", None, None)
        .await
        .unwrap();
    let docs = store.list_documents("ns", 10, 0).await.unwrap();

    // Create episodes with different outcomes
    let pending = EpisodeMeta {
        cause_ids: vec![],
        effect_type: "test_failure".to_string(),
        outcome: EpisodeOutcome::Pending,
        confidence: 0.5,
        verification_status: VerificationStatus::Unverified,
        experiment_id: None,
    };
    let confirmed = EpisodeMeta {
        cause_ids: vec![],
        effect_type: "regression".to_string(),
        outcome: EpisodeOutcome::Confirmed,
        confidence: 0.9,
        verification_status: VerificationStatus::Unverified,
        experiment_id: None,
    };

    store.ingest_episode(&docs[0].id, &pending).await.unwrap();
    store.ingest_episode(&docs[1].id, &confirmed).await.unwrap();

    // Search for confirmed only
    let results = store
        .search_episodes(None, Some(&EpisodeOutcome::Confirmed), 10)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].1.outcome, EpisodeOutcome::Confirmed);
}

// ─── Explainable Search Tests ─────────────────────────────────

#[tokio::test]
async fn test_explain_search_returns_breakdowns() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    store
        .add_fact("general", "Rust has zero-cost abstractions", None, None)
        .await
        .unwrap();
    store
        .add_fact("general", "Rust ensures memory safety", None, None)
        .await
        .unwrap();

    let explained: Vec<ExplainedResult> = store
        .search_explained("Rust features", None, None, None)
        .await
        .unwrap();

    assert!(!explained.is_empty(), "Should find results");

    // Each result should have a breakdown
    for ex in &explained {
        let b: &ScoreBreakdown = &ex.breakdown;
        assert!(b.rrf_score > 0.0, "RRF score should be positive");
        // At least one of bm25 or vector should be present
        assert!(
            b.bm25_rank.is_some() || b.vector_rank.is_some(),
            "At least one ranking component should be present"
        );
    }
}

// ─── Embedding Displacement Tests ─────────────────────────────

#[tokio::test]
async fn test_embedding_displacement_from_vecs() {
    // Identical vectors
    let a = vec![1.0f32, 0.0, 0.0];
    let b = vec![1.0f32, 0.0, 0.0];
    let d: EmbeddingDisplacement = MemoryStore::embedding_displacement_from_vecs(&a, &b).unwrap();
    assert!((d.cosine_similarity - 1.0).abs() < 0.01);
    assert!(d.euclidean_distance < 0.01);

    // Orthogonal vectors
    let a = vec![1.0f32, 0.0, 0.0];
    let b = vec![0.0f32, 1.0, 0.0];
    let d = MemoryStore::embedding_displacement_from_vecs(&a, &b).unwrap();
    assert!(d.cosine_similarity.abs() < 0.01);
    assert!((d.euclidean_distance - std::f32::consts::SQRT_2).abs() < 0.01);
}

#[tokio::test]
async fn test_embedding_displacement_dimension_mismatch_is_error() {
    let err = MemoryStore::embedding_displacement_from_vecs(&[1.0f32, 0.0], &[1.0f32]).unwrap_err();
    assert_eq!(err.kind(), "dimension_mismatch");
}

#[tokio::test]
async fn test_embedding_displacement_async() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    // MockEmbedder produces deterministic embeddings based on text hash
    let d = store
        .embedding_displacement("hello world", "hello world")
        .await
        .unwrap();
    // Same text should produce identical embeddings
    assert!((d.cosine_similarity - 1.0).abs() < 0.01);
    assert!(d.euclidean_distance < 0.01);
}
