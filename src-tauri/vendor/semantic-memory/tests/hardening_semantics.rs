#![allow(clippy::expect_used)]

use semantic_memory::{
    EpisodeMeta, EpisodeOutcome, GraphDirection, MemoryConfig, MemoryStore, MockEmbedder,
    SearchConfig, SearchSource, SearchSourceType, VerificationStatus,
};
#[cfg(feature = "testing")]
use semantic_memory::{ReconcileAction, Role, VerifyMode};
use tempfile::TempDir;

fn open_store(dir: &TempDir) -> MemoryStore {
    let config = MemoryConfig {
        base_dir: dir.path().to_path_buf(),
        search: SearchConfig {
            bm25_weight: 2.0,
            vector_weight: 3.0,
            rrf_k: 10.0,
            recency_half_life_days: Some(30.0),
            recency_weight: 0.5,
            min_similarity: -1.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let embedder = Box::new(MockEmbedder::new(config.embedding.dimensions));
    MemoryStore::open_with_embedder(config, embedder).expect("open store")
}

fn approx_eq(left: f64, right: f64, epsilon: f64) {
    assert!(
        (left - right).abs() <= epsilon,
        "left={left} right={right} epsilon={epsilon}"
    );
}

#[tokio::test]
async fn explainable_search_matches_configured_rrf_math() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    store
        .add_fact("general", "fusion proof fact", None, None)
        .await
        .unwrap();

    let explained = store
        .search_explained(
            "fusion proof fact",
            Some(1),
            None,
            Some(&[SearchSourceType::Facts]),
        )
        .await
        .unwrap();

    let top = &explained[0];
    let breakdown = &top.breakdown;
    let expected_bm25 = 2.0 / 11.0;
    let expected_vector = 3.0 / 11.0;
    let expected_recency = 0.5 / 11.0;

    assert_eq!(breakdown.bm25_rank, Some(1));
    assert_eq!(breakdown.vector_rank, Some(1));
    approx_eq(breakdown.bm25_contribution.unwrap(), expected_bm25, 1e-6);
    approx_eq(
        breakdown.vector_contribution.unwrap(),
        expected_vector,
        1e-6,
    );
    approx_eq(breakdown.recency_score.unwrap(), expected_recency, 5e-3);
    approx_eq(
        breakdown.rrf_score,
        expected_bm25 + expected_vector + breakdown.recency_score.unwrap(),
        5e-3,
    );
    approx_eq(top.result.score, breakdown.rrf_score, 1e-9);
    assert_eq!(breakdown.bm25_weight, 2.0);
    assert_eq!(breakdown.vector_weight, 3.0);
    assert_eq!(breakdown.rrf_k, 10.0);
}

#[tokio::test]
async fn episodes_participate_in_generic_search_and_explanations() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    store
        .ingest_document(
            "Auth Incident",
            "The auth module regression caused repeated login failures.",
            "ops",
            None,
            None,
        )
        .await
        .unwrap();
    let document_id = store.list_documents("ops", 10, 0).await.unwrap()[0]
        .id
        .clone();

    store
        .ingest_episode(
            &document_id,
            &EpisodeMeta {
                cause_ids: vec!["fact-auth-1".to_string()],
                effect_type: "regression".to_string(),
                outcome: EpisodeOutcome::Pending,
                confidence: 0.8,
                verification_status: VerificationStatus::Unverified,
                experiment_id: None,
            },
        )
        .await
        .unwrap();

    let results = store
        .search(
            "login failure regression",
            Some(5),
            Some(&["ops"]),
            Some(&[SearchSourceType::Episodes]),
        )
        .await
        .unwrap();
    assert!(!results.is_empty());
    assert!(matches!(
        &results[0].source,
        SearchSource::Episode { document_id: id, .. } if id == &document_id
    ));

    let explained = store
        .search_explained(
            "login failure regression",
            Some(5),
            Some(&["ops"]),
            Some(&[SearchSourceType::Episodes]),
        )
        .await
        .unwrap();
    assert!(!explained.is_empty());
    approx_eq(
        explained[0].result.score,
        explained[0].breakdown.rrf_score,
        1e-9,
    );
}

#[tokio::test]
async fn graph_view_exposes_document_chunk_and_episode_causal_links() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    let fact_id = store
        .add_fact("ops", "auth service timeout cause", None, None)
        .await
        .unwrap();
    store
        .ingest_document(
            "Incident Report",
            "The report describes the auth service timeout and downstream login failures.",
            "ops",
            None,
            None,
        )
        .await
        .unwrap();
    let document_id = store.list_documents("ops", 10, 0).await.unwrap()[0]
        .id
        .clone();
    let episode_id = store
        .ingest_episode(
            &document_id,
            &EpisodeMeta {
                cause_ids: vec![fact_id.clone()],
                effect_type: "incident".to_string(),
                outcome: EpisodeOutcome::Confirmed,
                confidence: 0.9,
                verification_status: VerificationStatus::Verified {
                    method: "manual".to_string(),
                    at: "2026-03-06 00:00:00".to_string(),
                },
                experiment_id: Some("exp-incident".to_string()),
            },
        )
        .await
        .unwrap();

    let graph = store.graph_view();
    let doc_edges = graph
        .neighbors(
            &format!("document:{document_id}"),
            GraphDirection::Outgoing,
            1,
        )
        .unwrap();
    assert!(doc_edges
        .iter()
        .any(|edge| edge.target.starts_with("chunk:")));
    assert!(doc_edges
        .iter()
        .any(|edge| edge.target == format!("episode:{episode_id}")));

    let path = graph
        .path(
            &format!("episode:{episode_id}"),
            &format!("fact:{fact_id}"),
            2,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        path,
        vec![format!("episode:{episode_id}"), format!("fact:{fact_id}")]
    );
}

#[cfg(feature = "testing")]
#[tokio::test]
async fn corruption_is_visible_in_live_reads_and_integrity_reports() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    let session_id = store.create_session("chat").await.unwrap();
    let message_id = store
        .add_message_fts(&session_id, Role::User, "corruption seam", None, None)
        .await
        .unwrap();

    store
        .raw_execute(
            "UPDATE messages SET metadata = ?1 WHERE id = ?2",
            vec!["{not-json".to_string(), message_id.to_string()],
        )
        .await
        .unwrap();

    let err = store
        .get_recent_messages(&session_id, 10)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "corrupt_data");

    let report = store.verify_integrity(VerifyMode::Full).await.unwrap();
    assert!(report
        .issues
        .iter()
        .any(|issue| issue.contains("invalid metadata")));
}

#[cfg(feature = "testing")]
#[tokio::test]
async fn reconcile_reembed_repairs_missing_quantized_embeddings() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    store
        .add_fact("ops", "quantized repair target", None, None)
        .await
        .unwrap();

    store
        .raw_execute("UPDATE facts SET embedding_q8 = NULL", vec![])
        .await
        .unwrap();

    let before = store.verify_integrity(VerifyMode::Full).await.unwrap();
    assert!(before
        .issues
        .iter()
        .any(|issue| issue.contains("missing quantized embedding")));

    let after = store.reconcile(ReconcileAction::ReEmbed).await.unwrap();
    assert!(
        after.ok,
        "reconcile should repair missing q8 blobs: {:?}",
        after.issues
    );
}
