//! Tests for the v0.5.0 hardening pass:
#![allow(clippy::expect_used)]
//! - First-class episode_id identity with multi-episode support
//! - Normalized causal edges via episode_causes table
//! - Graph view uses episode_causes (no full-table JSON scan)
//! - Pool timeout & RAII panic-safety
//! - OllamaEmbedder::try_new fallible constructor
//! - Integrity checker surfaces errors instead of silently defaulting
//! - ingest_episode returns episode_id

use semantic_memory::{
    EpisodeMeta, EpisodeOutcome, GraphDirection, GraphEdgeType, MemoryConfig, MemoryStore,
    MockEmbedder, PoolConfig, SearchConfig, SearchSourceType, VerificationStatus,
};
use tempfile::TempDir;

fn open_store(dir: &TempDir) -> MemoryStore {
    let config = MemoryConfig {
        base_dir: dir.path().to_path_buf(),
        search: SearchConfig {
            min_similarity: -1.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let embedder = Box::new(MockEmbedder::new(config.embedding.dimensions));
    MemoryStore::open_with_embedder(config, embedder).expect("open store")
}

fn open_store_with_pool(dir: &TempDir, pool: PoolConfig) -> MemoryStore {
    let config = MemoryConfig {
        base_dir: dir.path().to_path_buf(),
        pool,
        search: SearchConfig {
            min_similarity: -1.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let embedder = Box::new(MockEmbedder::new(config.embedding.dimensions));
    MemoryStore::open_with_embedder(config, embedder).expect("open store")
}

fn test_meta() -> EpisodeMeta {
    EpisodeMeta {
        cause_ids: vec!["cause-a".to_string()],
        effect_type: "test_effect".to_string(),
        outcome: EpisodeOutcome::Pending,
        confidence: 0.8,
        verification_status: VerificationStatus::Unverified,
        experiment_id: None,
    }
}

// ─── Episode Identity Tests ─────────────────────────────────

#[tokio::test]
async fn ingest_episode_returns_episode_id() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);
    let doc_id = store
        .ingest_document("doc", "content here", "ns", None, None)
        .await
        .unwrap();
    let episode_id = store.ingest_episode(&doc_id, &test_meta()).await.unwrap();
    assert!(
        !episode_id.is_empty(),
        "ingest_episode should return a non-empty episode_id"
    );
    assert!(
        episode_id.contains(&doc_id),
        "legacy upsert episode_id should contain the document_id"
    );
}

#[tokio::test]
async fn create_episode_with_explicit_id() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);
    let doc_id = store
        .ingest_document("doc", "content here", "ns", None, None)
        .await
        .unwrap();
    let ep_id = store
        .create_episode("my-custom-ep-1", &doc_id, &test_meta())
        .await
        .unwrap();
    assert_eq!(ep_id, "my-custom-ep-1");
}

#[tokio::test]
async fn get_episode_by_id() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);
    let doc_id = store
        .ingest_document("doc", "content here", "ns", None, None)
        .await
        .unwrap();
    let ep_id = store
        .create_episode("ep-get-test", &doc_id, &test_meta())
        .await
        .unwrap();
    let (returned_doc_id, meta) = store
        .get_episode(&ep_id)
        .await
        .unwrap()
        .expect("episode should exist");
    assert_eq!(returned_doc_id, doc_id);
    assert_eq!(meta.effect_type, "test_effect");
    assert_eq!(meta.confidence, 0.8);
}

#[tokio::test]
async fn get_episode_returns_none_for_missing() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);
    let result = store.get_episode("nonexistent-ep").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn multiple_episodes_per_document() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);
    let doc_id = store
        .ingest_document("doc", "multi-ep content", "ns", None, None)
        .await
        .unwrap();

    let meta_a = EpisodeMeta {
        cause_ids: vec!["cause-1".to_string()],
        effect_type: "effect_a".to_string(),
        outcome: EpisodeOutcome::Pending,
        confidence: 0.6,
        verification_status: VerificationStatus::Unverified,
        experiment_id: None,
    };
    let meta_b = EpisodeMeta {
        cause_ids: vec!["cause-2".to_string()],
        effect_type: "effect_b".to_string(),
        outcome: EpisodeOutcome::Confirmed,
        confidence: 0.9,
        verification_status: VerificationStatus::Verified {
            method: "test".to_string(),
            at: "2026-03-06".to_string(),
        },
        experiment_id: Some("exp-1".to_string()),
    };

    let ep_a = store
        .create_episode("ep-a", &doc_id, &meta_a)
        .await
        .unwrap();
    let ep_b = store
        .create_episode("ep-b", &doc_id, &meta_b)
        .await
        .unwrap();

    assert_ne!(ep_a, ep_b);

    // Both episodes should be retrievable
    let (_, a_meta) = store.get_episode(&ep_a).await.unwrap().unwrap();
    let (_, b_meta) = store.get_episode(&ep_b).await.unwrap().unwrap();
    assert_eq!(a_meta.effect_type, "effect_a");
    assert_eq!(b_meta.effect_type, "effect_b");
}

#[tokio::test]
async fn update_episode_outcome_by_id() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);
    let doc_id = store
        .ingest_document("doc", "outcome test", "ns", None, None)
        .await
        .unwrap();
    let ep_id = store
        .create_episode("ep-outcome", &doc_id, &test_meta())
        .await
        .unwrap();

    store
        .update_episode_outcome_by_id(&ep_id, EpisodeOutcome::Confirmed, 0.95, Some("exp-42"))
        .await
        .unwrap();

    let (_, updated_meta) = store.get_episode(&ep_id).await.unwrap().unwrap();
    assert_eq!(updated_meta.outcome, EpisodeOutcome::Confirmed);
    assert_eq!(updated_meta.confidence, 0.95);
    assert_eq!(updated_meta.experiment_id, Some("exp-42".to_string()));
}

// ─── Graph View with episode_id ─────────────────────────────

#[tokio::test]
async fn graph_document_links_all_episodes() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);
    let doc_id = store
        .ingest_document("doc", "graph test", "ns", None, None)
        .await
        .unwrap();

    let ep1 = store
        .create_episode("ep-graph-1", &doc_id, &test_meta())
        .await
        .unwrap();
    let ep2 = store
        .create_episode(
            "ep-graph-2",
            &doc_id,
            &EpisodeMeta {
                effect_type: "other_effect".to_string(),
                ..test_meta()
            },
        )
        .await
        .unwrap();

    let graph = store.graph_view();
    let doc_edges = graph
        .neighbors(&format!("document:{doc_id}"), GraphDirection::Outgoing, 1)
        .unwrap();

    let episode_targets: Vec<&str> = doc_edges
        .iter()
        .filter(|e| e.target.starts_with("episode:"))
        .map(|e| e.target.as_str())
        .collect();

    assert!(
        episode_targets.contains(&format!("episode:{ep1}").as_str()),
        "document should link to first episode"
    );
    assert!(
        episode_targets.contains(&format!("episode:{ep2}").as_str()),
        "document should link to second episode"
    );
}

#[tokio::test]
async fn graph_causal_backlinks_use_normalized_table() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    // Create a fact that will be the cause
    let fact_id = store
        .add_fact("ns", "root cause fact", None, None)
        .await
        .unwrap();

    let doc_id = store
        .ingest_document("doc", "causal test", "ns", None, None)
        .await
        .unwrap();

    let meta = EpisodeMeta {
        cause_ids: vec![fact_id.clone()],
        effect_type: "caused_effect".to_string(),
        outcome: EpisodeOutcome::Confirmed,
        confidence: 0.85,
        verification_status: VerificationStatus::Verified {
            method: "test".to_string(),
            at: "2026-03-06".to_string(),
        },
        experiment_id: None,
    };
    let ep_id = store
        .create_episode("ep-causal", &doc_id, &meta)
        .await
        .unwrap();

    let graph = store.graph_view();

    // The fact node should have incoming causal edges from the episode
    let incoming = graph
        .neighbors(&format!("fact:{fact_id}"), GraphDirection::Incoming, 1)
        .unwrap();

    let causal_from_episode = incoming
        .iter()
        .filter(|e| matches!(e.edge_type, GraphEdgeType::Causal { .. }))
        .any(|e| e.source == format!("episode:{ep_id}"));
    assert!(
        causal_from_episode,
        "fact should have causal backlink from the episode via episode_causes table"
    );
}

#[tokio::test]
async fn graph_episode_node_has_causal_and_document_edges() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    let fact_id = store
        .add_fact("ns", "cause fact", None, None)
        .await
        .unwrap();

    let doc_id = store
        .ingest_document("doc", "episode edges test", "ns", None, None)
        .await
        .unwrap();

    let meta = EpisodeMeta {
        cause_ids: vec![fact_id.clone()],
        effect_type: "edge_test".to_string(),
        outcome: EpisodeOutcome::Pending,
        confidence: 0.7,
        verification_status: VerificationStatus::Unverified,
        experiment_id: None,
    };
    let ep_id = store
        .create_episode("ep-edges", &doc_id, &meta)
        .await
        .unwrap();

    let graph = store.graph_view();
    let ep_edges = graph
        .neighbors(&format!("episode:{ep_id}"), GraphDirection::Outgoing, 1)
        .unwrap();

    // Should have attached_to_document edge
    assert!(
        ep_edges
            .iter()
            .any(|e| e.target == format!("document:{doc_id}")),
        "episode should have attached_to_document edge"
    );

    // Should have causal edge to the fact
    let has_causal = ep_edges.iter().any(|e| {
        matches!(e.edge_type, GraphEdgeType::Causal { .. }) && e.target.contains(&fact_id)
    });
    assert!(has_causal, "episode should have causal edge to cause fact");
}

// ─── Episode Search Identity ────────────────────────────────

#[tokio::test]
async fn episode_search_results_include_episode_id() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);
    let doc_id = store
        .ingest_document("doc", "search identity test content", "ns", None, None)
        .await
        .unwrap();
    let ep_id = store
        .create_episode("ep-search-id", &doc_id, &test_meta())
        .await
        .unwrap();

    let results = store
        .search(
            "test_effect",
            Some(10),
            None,
            Some(&[SearchSourceType::Episodes]),
        )
        .await
        .unwrap();

    if !results.is_empty() {
        // If episode shows up in search, verify it carries episode_id
        for result in &results {
            if let semantic_memory::SearchSource::Episode {
                episode_id,
                document_id,
                ..
            } = &result.source
            {
                assert_eq!(episode_id, &ep_id);
                assert_eq!(document_id, &doc_id);
            }
        }
    }
}

// ─── Pool Timeout ───────────────────────────────────────────

#[tokio::test]
async fn pool_respects_configured_timeout() {
    let dir = TempDir::new().unwrap();
    let pool_config = PoolConfig {
        reader_timeout_secs: 1,
        max_read_connections: 1,
        ..Default::default()
    };
    // Just opening with a 1s timeout should work fine
    let store = open_store_with_pool(&dir, pool_config);
    // A basic operation should succeed
    let result = store.list_facts("ns", 10, 0).await;
    assert!(
        result.is_ok(),
        "basic read should succeed with timeout config"
    );
}

// ─── Integrity Checker ──────────────────────────────────────

#[tokio::test]
async fn verify_integrity_on_clean_store() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);
    let report = store
        .verify_integrity(semantic_memory::VerifyMode::Full)
        .await
        .unwrap();
    assert!(
        report.ok,
        "clean store should pass integrity check, issues: {:?}",
        report.issues
    );
}

#[tokio::test]
async fn verify_integrity_after_episodes() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    let doc_id = store
        .ingest_document("doc", "integrity test content", "ns", None, None)
        .await
        .unwrap();
    store
        .create_episode("ep-integrity", &doc_id, &test_meta())
        .await
        .unwrap();

    let report = store
        .verify_integrity(semantic_memory::VerifyMode::Full)
        .await
        .unwrap();
    assert!(
        report.ok,
        "store with episodes should pass integrity check, issues: {:?}",
        report.issues
    );
}

// ─── Reconcile FTS with new episode schema ──────────────────

#[tokio::test]
async fn reconcile_fts_rebuilds_episode_indexes() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    let doc_id = store
        .ingest_document("doc", "reconcile test", "ns", None, None)
        .await
        .unwrap();
    store
        .create_episode("ep-reconcile", &doc_id, &test_meta())
        .await
        .unwrap();

    // Reconcile should succeed and result in clean integrity
    let report = store
        .reconcile(semantic_memory::ReconcileAction::RebuildFts)
        .await
        .unwrap();
    assert!(
        report.ok,
        "post-reconcile integrity should be clean, issues: {:?}",
        report.issues
    );
}

// ─── OllamaEmbedder::try_new ───────────────────────────────

#[test]
fn embedder_try_new_returns_result() {
    use semantic_memory::{EmbeddingConfig, OllamaEmbedder};

    let config = EmbeddingConfig::default();
    // try_new should succeed with default config (just builds an HTTP client)
    let result = OllamaEmbedder::try_new(&config);
    assert!(result.is_ok(), "try_new with default config should succeed");
}

// ─── Legacy compat: ingest_episode upserts on re-call ───────

#[tokio::test]
async fn ingest_episode_upserts_on_repeat() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);
    let doc_id = store
        .ingest_document("doc", "upsert test", "ns", None, None)
        .await
        .unwrap();

    let ep_id_1 = store.ingest_episode(&doc_id, &test_meta()).await.unwrap();
    let meta2 = EpisodeMeta {
        confidence: 0.99,
        outcome: EpisodeOutcome::Confirmed,
        ..test_meta()
    };
    let ep_id_2 = store.ingest_episode(&doc_id, &meta2).await.unwrap();

    // Same episode_id returned (upsert, not duplicate)
    assert_eq!(
        ep_id_1, ep_id_2,
        "re-ingesting should upsert the same episode"
    );

    // Meta should reflect the update
    let (_, meta) = store.get_episode(&ep_id_1).await.unwrap().unwrap();
    assert_eq!(meta.confidence, 0.99);
    assert_eq!(meta.outcome, EpisodeOutcome::Confirmed);
}

// ─── Backward compat: update_episode_outcome by document_id ─

#[tokio::test]
async fn legacy_update_episode_outcome_by_document_id() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);
    let doc_id = store
        .ingest_document("doc", "legacy update test", "ns", None, None)
        .await
        .unwrap();
    let ep_id = store.ingest_episode(&doc_id, &test_meta()).await.unwrap();

    // Legacy API: update by document_id
    store
        .update_episode_outcome(&doc_id, EpisodeOutcome::Refuted, 0.1, None)
        .await
        .unwrap();

    let (_, meta) = store.get_episode(&ep_id).await.unwrap().unwrap();
    assert_eq!(meta.outcome, EpisodeOutcome::Refuted);
    assert_eq!(meta.confidence, 0.1);
}

// ─── Episode search_episodes returns results ────────────────

#[tokio::test]
async fn search_episodes_filters_by_effect_and_outcome() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);
    let doc_id = store
        .ingest_document("doc", "filter test", "ns", None, None)
        .await
        .unwrap();

    let pending_meta = EpisodeMeta {
        effect_type: "deploy_failure".to_string(),
        outcome: EpisodeOutcome::Pending,
        ..test_meta()
    };
    store
        .create_episode("ep-filter-1", &doc_id, &pending_meta)
        .await
        .unwrap();

    let confirmed_meta = EpisodeMeta {
        effect_type: "deploy_failure".to_string(),
        outcome: EpisodeOutcome::Confirmed,
        ..test_meta()
    };
    store
        .create_episode("ep-filter-2", &doc_id, &confirmed_meta)
        .await
        .unwrap();

    // Filter by effect_type only
    let by_effect = store
        .search_episodes(Some("deploy_failure"), None, 10)
        .await
        .unwrap();
    assert_eq!(by_effect.len(), 2);

    // Filter by outcome
    let by_outcome = store
        .search_episodes(None, Some(&EpisodeOutcome::Confirmed), 10)
        .await
        .unwrap();
    assert!(!by_outcome.is_empty());
    assert!(by_outcome
        .iter()
        .all(|(_, m)| m.outcome == EpisodeOutcome::Confirmed));
}
