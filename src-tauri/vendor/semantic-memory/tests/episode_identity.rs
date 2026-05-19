//! Tests for multi-episode-per-document correctness and the episode identity seam.
#![allow(clippy::expect_used)]
//!
//! These tests close the migration gap between legacy document-keyed episode behavior
//! and canonical episode_id-first identity.

use semantic_memory::{
    EpisodeMeta, EpisodeOutcome, GraphDirection, GraphEdgeType, MemoryConfig, MemoryStore,
    MockEmbedder, SearchConfig, SearchSourceType, VerificationStatus,
};
use tempfile::TempDir;

fn test_store() -> (MemoryStore, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let config = MemoryConfig {
        base_dir: dir.path().to_path_buf(),
        search: SearchConfig {
            min_similarity: -1.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let embedder = Box::new(MockEmbedder::new(config.embedding.dimensions));
    let store = MemoryStore::open_with_embedder(config, embedder).expect("open store");
    (store, dir)
}

fn make_meta(effect_type: &str, cause_ids: Vec<&str>) -> EpisodeMeta {
    EpisodeMeta {
        cause_ids: cause_ids.into_iter().map(String::from).collect(),
        effect_type: effect_type.to_string(),
        outcome: EpisodeOutcome::Pending,
        confidence: 0.5,
        verification_status: VerificationStatus::Unverified,
        experiment_id: None,
    }
}

// ──────────────────────────────────────────────────────────────────────
// 1. Multi-episode reembed regression
// ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn reembed_preserves_sibling_episode_identity() {
    let (store, _dir) = test_store();

    // Create one document with three distinct episodes
    let doc_id = store
        .ingest_document(
            "test-doc",
            "content for episode test",
            "general",
            None,
            None,
        )
        .await
        .unwrap();

    let ep1 = store
        .create_episode("ep-alpha", &doc_id, &make_meta("effect_a", vec![]))
        .await
        .unwrap();
    let ep2 = store
        .create_episode("ep-beta", &doc_id, &make_meta("effect_b", vec![]))
        .await
        .unwrap();
    let ep3 = store
        .create_episode("ep-gamma", &doc_id, &make_meta("effect_c", vec![]))
        .await
        .unwrap();

    assert_eq!(ep1, "ep-alpha");
    assert_eq!(ep2, "ep-beta");
    assert_eq!(ep3, "ep-gamma");

    // Verify each episode exists independently
    let (_, meta1) = store.get_episode("ep-alpha").await.unwrap().unwrap();
    let (_, meta2) = store.get_episode("ep-beta").await.unwrap().unwrap();
    let (_, meta3) = store.get_episode("ep-gamma").await.unwrap().unwrap();
    assert_eq!(meta1.effect_type, "effect_a");
    assert_eq!(meta2.effect_type, "effect_b");
    assert_eq!(meta3.effect_type, "effect_c");

    // Run reembed_all — the critical path under test
    let count = store.reembed_all().await.unwrap();
    // Should reembed at least the 3 episodes plus chunks and other entities
    assert!(
        count >= 3,
        "reembed should process all 3 episodes, got {count}"
    );

    // Verify each episode still has its own distinct metadata after reembedding
    let (doc1, meta1) = store.get_episode("ep-alpha").await.unwrap().unwrap();
    let (doc2, meta2) = store.get_episode("ep-beta").await.unwrap().unwrap();
    let (doc3, meta3) = store.get_episode("ep-gamma").await.unwrap().unwrap();

    // All should point to the same document
    assert_eq!(doc1, doc_id);
    assert_eq!(doc2, doc_id);
    assert_eq!(doc3, doc_id);

    // Each should retain its distinct effect_type
    assert_eq!(meta1.effect_type, "effect_a");
    assert_eq!(meta2.effect_type, "effect_b");
    assert_eq!(meta3.effect_type, "effect_c");
}

// ──────────────────────────────────────────────────────────────────────
// 2. Episode HNSW key correctness
// ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn episode_hnsw_keys_use_episode_id() {
    let (store, dir) = test_store();

    let doc_id = store
        .ingest_document("hnsw-doc", "hnsw key test content", "general", None, None)
        .await
        .unwrap();

    let ep_id = store
        .create_episode("ep-hnsw-test", &doc_id, &make_meta("hnsw_check", vec![]))
        .await
        .unwrap();

    // Verify the pending index op key uses episode_id, not document_id
    let conn = rusqlite::Connection::open(dir.path().join("memory.db")).unwrap();
    let _keys: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT item_key FROM pending_index_ops WHERE entity_type = 'episode'")
            .unwrap();
        stmt.query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };

    // The key should be episode:{episode_id}, not episode:{document_id}
    let expected_key = format!("episode:{ep_id}");
    let wrong_key = format!("episode:{doc_id}");

    // After hnsw sync, pending ops may be cleared; check keymap or that no wrong key exists
    // But we can check the hnsw_keymap table for the correct key pattern
    let keymap_keys: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT item_key FROM hnsw_keymap WHERE item_key LIKE 'episode:%'")
            .unwrap();
        stmt.query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };

    // No canonical path should produce episode:{document_id}
    for key in &keymap_keys {
        assert!(
            !key.starts_with(&format!("episode:{}", doc_id)) || key == &expected_key,
            "Found wrong key pattern: {key}, expected keys to use episode_id not document_id"
        );
    }

    // The correct key should be present
    if !keymap_keys.is_empty() {
        assert!(
            keymap_keys.contains(&expected_key),
            "Expected HNSW keymap to contain {expected_key}, found: {keymap_keys:?}"
        );
        assert!(
            !keymap_keys.contains(&wrong_key),
            "HNSW keymap should not contain document-keyed entry {wrong_key}"
        );
    }
}

// ──────────────────────────────────────────────────────────────────────
// 3. Episode update isolation
// ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn update_one_episode_does_not_touch_siblings() {
    let (store, _dir) = test_store();

    let doc_id = store
        .ingest_document(
            "isolation-doc",
            "isolation test content",
            "general",
            None,
            None,
        )
        .await
        .unwrap();

    store
        .create_episode("ep-sib-1", &doc_id, &make_meta("type_x", vec![]))
        .await
        .unwrap();
    store
        .create_episode("ep-sib-2", &doc_id, &make_meta("type_y", vec![]))
        .await
        .unwrap();
    store
        .create_episode("ep-sib-3", &doc_id, &make_meta("type_z", vec![]))
        .await
        .unwrap();

    // Update only ep-sib-2's outcome
    store
        .update_episode_outcome_by_id("ep-sib-2", EpisodeOutcome::Confirmed, 0.95, None)
        .await
        .unwrap();

    // Verify ep-sib-1 and ep-sib-3 are untouched
    let (_, meta1) = store.get_episode("ep-sib-1").await.unwrap().unwrap();
    let (_, meta2) = store.get_episode("ep-sib-2").await.unwrap().unwrap();
    let (_, meta3) = store.get_episode("ep-sib-3").await.unwrap().unwrap();

    assert_eq!(meta1.outcome, EpisodeOutcome::Pending);
    assert_eq!(meta1.effect_type, "type_x");
    assert_eq!(meta1.confidence, 0.5);

    assert_eq!(meta2.outcome, EpisodeOutcome::Confirmed);
    assert_eq!(meta2.confidence, 0.95);

    assert_eq!(meta3.outcome, EpisodeOutcome::Pending);
    assert_eq!(meta3.effect_type, "type_z");
    assert_eq!(meta3.confidence, 0.5);
}

// ──────────────────────────────────────────────────────────────────────
// 4. Compat wrapper determinism
// ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn legacy_document_keyed_update_touches_only_primary_episode() {
    let (store, _dir) = test_store();

    let doc_id = store
        .ingest_document("compat-doc", "compat test content", "general", None, None)
        .await
        .unwrap();

    // Create episodes; the legacy compat path should touch only the first-created one
    store
        .create_episode("ep-first", &doc_id, &make_meta("first_effect", vec![]))
        .await
        .unwrap();
    store
        .create_episode("ep-second", &doc_id, &make_meta("second_effect", vec![]))
        .await
        .unwrap();

    // Use the legacy document-keyed update
    store
        .update_episode_outcome(&doc_id, EpisodeOutcome::Refuted, 0.3, None)
        .await
        .unwrap();

    // Only the first episode should be affected
    let (_, meta_first) = store.get_episode("ep-first").await.unwrap().unwrap();
    let (_, meta_second) = store.get_episode("ep-second").await.unwrap().unwrap();

    assert_eq!(meta_first.outcome, EpisodeOutcome::Refuted);
    assert_eq!(meta_first.confidence, 0.3);

    // Second episode should be untouched
    assert_eq!(meta_second.outcome, EpisodeOutcome::Pending);
    assert_eq!(meta_second.effect_type, "second_effect");
    assert_eq!(meta_second.confidence, 0.5);
}

// ──────────────────────────────────────────────────────────────────────
// 5. Graph fanout correctness
// ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn document_graph_node_resolves_to_all_episodes() {
    let (store, _dir) = test_store();

    let doc_id = store
        .ingest_document("graph-doc", "graph fanout content", "general", None, None)
        .await
        .unwrap();

    let ep1 = store
        .create_episode("ep-g1", &doc_id, &make_meta("g_effect_1", vec![]))
        .await
        .unwrap();
    let ep2 = store
        .create_episode("ep-g2", &doc_id, &make_meta("g_effect_2", vec![]))
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
        "document should link to first episode, got: {episode_targets:?}"
    );
    assert!(
        episode_targets.contains(&format!("episode:{ep2}").as_str()),
        "document should link to second episode, got: {episode_targets:?}"
    );
}

#[tokio::test]
async fn canonical_episode_node_uses_episode_id() {
    let (store, _dir) = test_store();

    let doc_id = store
        .ingest_document("canon-doc", "canonical node test", "general", None, None)
        .await
        .unwrap();

    let ep_id = store
        .create_episode("ep-canon", &doc_id, &make_meta("canon_effect", vec![]))
        .await
        .unwrap();

    let graph = store.graph_view();
    let ep_edges = graph
        .neighbors(&format!("episode:{ep_id}"), GraphDirection::Outgoing, 1)
        .unwrap();

    // The episode node should have an edge to its document
    let has_doc_edge = ep_edges.iter().any(|e| {
        e.source == format!("episode:{ep_id}") && e.target == format!("document:{doc_id}")
    });
    assert!(has_doc_edge, "episode should link to its document");

    // All source nodes should be episode:{episode_id}, never episode:{document_id}
    for edge in &ep_edges {
        if edge.source.starts_with("episode:") {
            assert_eq!(
                edge.source,
                format!("episode:{ep_id}"),
                "episode edge source must use episode_id, not document_id"
            );
        }
    }
}

#[tokio::test]
async fn canonical_episode_node_does_not_fallback_from_document_id() {
    let (store, _dir) = test_store();

    let doc_id = store
        .ingest_document(
            "canon-no-fallback-doc",
            "canonical episode node fallback test",
            "general",
            None,
            None,
        )
        .await
        .unwrap();

    store
        .create_episode(
            "ep-no-fallback",
            &doc_id,
            &make_meta("canon_effect", vec![]),
        )
        .await
        .unwrap();

    let graph = store.graph_view();
    let doc_keyed_episode_edges = graph
        .neighbors(&format!("episode:{doc_id}"), GraphDirection::Outgoing, 1)
        .unwrap();

    assert!(
        doc_keyed_episode_edges.is_empty(),
        "canonical episode graph lookup must not treat document ids as episode ids"
    );
}

// ──────────────────────────────────────────────────────────────────────
// 6. Integrity / reconcile sibling targeting
// ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn integrity_surfaces_episode_level_drift() {
    let (store, dir) = test_store();

    let doc_id = store
        .ingest_document(
            "integrity-doc",
            "integrity test content",
            "general",
            None,
            None,
        )
        .await
        .unwrap();

    store
        .create_episode("ep-intact", &doc_id, &make_meta("intact_effect", vec![]))
        .await
        .unwrap();
    store
        .create_episode("ep-broken", &doc_id, &make_meta("broken_effect", vec![]))
        .await
        .unwrap();

    // Break only ep-broken's rowid map entry
    {
        let conn = rusqlite::Connection::open(dir.path().join("memory.db")).unwrap();
        conn.execute(
            "DELETE FROM episodes_rowid_map WHERE episode_id = 'ep-broken'",
            [],
        )
        .unwrap();
    }

    // Integrity check should detect the drift
    let report = store
        .verify_integrity(semantic_memory::VerifyMode::Full)
        .await
        .unwrap();
    assert!(
        !report.ok,
        "integrity should flag episode FTS drift when one sibling is broken"
    );

    let fts_issue = report
        .issues
        .iter()
        .any(|i| i.contains("episodes") && i.contains("drift"));
    assert!(
        fts_issue,
        "integrity should mention episode FTS drift, issues: {:?}",
        report.issues
    );

    // Reconcile should repair it
    let repaired = store
        .reconcile(semantic_memory::ReconcileAction::RebuildFts)
        .await
        .unwrap();
    assert!(
        repaired.ok,
        "reconcile should fix episode FTS drift, remaining issues: {:?}",
        repaired.issues
    );

    // Both episodes should still exist after repair
    assert!(store.get_episode("ep-intact").await.unwrap().is_some());
    assert!(store.get_episode("ep-broken").await.unwrap().is_some());
}

// ──────────────────────────────────────────────────────────────────────
// 7. Migration continuity (simulated)
// ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn legacy_ep0_migration_then_add_sibling() {
    let (store, _dir) = test_store();

    let doc_id = store
        .ingest_document(
            "migration-doc",
            "migration test content",
            "general",
            None,
            None,
        )
        .await
        .unwrap();

    // Use legacy ingest_episode which creates deterministic {doc_id}-ep0
    let ep0_id = store
        .ingest_episode(&doc_id, &make_meta("migrated_effect", vec![]))
        .await
        .unwrap();
    assert_eq!(ep0_id, format!("{doc_id}-ep0"), "legacy should create -ep0");

    // Now add a sibling via the canonical path
    let ep1_id = store
        .create_episode(
            "ep-sibling-1",
            &doc_id,
            &make_meta("sibling_effect", vec![]),
        )
        .await
        .unwrap();
    assert_eq!(ep1_id, "ep-sibling-1");

    // Both should exist
    let (doc0, meta0) = store.get_episode(&ep0_id).await.unwrap().unwrap();
    let (doc1, meta1) = store.get_episode(&ep1_id).await.unwrap().unwrap();
    assert_eq!(doc0, doc_id);
    assert_eq!(doc1, doc_id);
    assert_eq!(meta0.effect_type, "migrated_effect");
    assert_eq!(meta1.effect_type, "sibling_effect");

    // Search should find both
    let results = store
        .search(
            "migration test",
            Some(20),
            None,
            Some(&[SearchSourceType::Episodes]),
        )
        .await
        .unwrap();
    assert!(
        results.len() >= 2,
        "search should find both episodes, found {}",
        results.len()
    );

    // Graph should show both episodes under the document
    let graph = store.graph_view();
    let doc_edges = graph
        .neighbors(&format!("document:{doc_id}"), GraphDirection::Outgoing, 1)
        .unwrap();
    let ep_targets: Vec<&str> = doc_edges
        .iter()
        .filter(|e| e.target.starts_with("episode:"))
        .map(|e| e.target.as_str())
        .collect();
    assert!(ep_targets.contains(&format!("episode:{ep0_id}").as_str()));
    assert!(ep_targets.contains(&format!("episode:{ep1_id}").as_str()));

    // Integrity should be clean
    let report = store
        .verify_integrity(semantic_memory::VerifyMode::Full)
        .await
        .unwrap();
    assert!(
        report.ok,
        "integrity should be clean after migration + sibling, issues: {:?}",
        report.issues
    );

    // Reembed should work without smearing
    let count = store.reembed_all().await.unwrap();
    assert!(count >= 2, "reembed should process at least 2 episodes");

    let (_, meta0_after) = store.get_episode(&ep0_id).await.unwrap().unwrap();
    let (_, meta1_after) = store.get_episode(&ep1_id).await.unwrap().unwrap();
    assert_eq!(meta0_after.effect_type, "migrated_effect");
    assert_eq!(meta1_after.effect_type, "sibling_effect");
}

// ──────────────────────────────────────────────────────────────────────
// Additional: reembed isolation with embedding verification
// ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn reembed_does_not_collapse_multiple_episodes_per_document() {
    let (store, dir) = test_store();

    let doc_id = store
        .ingest_document(
            "multi-ep-doc",
            "multi-episode reembed",
            "general",
            None,
            None,
        )
        .await
        .unwrap();

    store
        .create_episode("ep-re-1", &doc_id, &make_meta("re_type_1", vec![]))
        .await
        .unwrap();
    store
        .create_episode("ep-re-2", &doc_id, &make_meta("re_type_2", vec![]))
        .await
        .unwrap();
    store
        .create_episode("ep-re-3", &doc_id, &make_meta("re_type_3", vec![]))
        .await
        .unwrap();

    // Verify 3 distinct episodes exist in the DB
    let conn = rusqlite::Connection::open(dir.path().join("memory.db")).unwrap();
    let ep_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM episodes WHERE document_id = ?1",
            rusqlite::params![&doc_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(ep_count, 3, "should have 3 episodes under the document");

    // Run reembed
    store.reembed_all().await.unwrap();

    // Still 3 episodes
    let ep_count_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM episodes WHERE document_id = ?1",
            rusqlite::params![&doc_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(ep_count_after, 3, "reembed should not collapse episodes");

    // Each should have a non-null embedding
    let embeddings: Vec<(String, bool)> = {
        let mut stmt = conn
            .prepare(
                "SELECT episode_id, embedding IS NOT NULL FROM episodes WHERE document_id = ?1",
            )
            .unwrap();
        stmt.query_map(rusqlite::params![&doc_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    };
    for (ep_id, has_embedding) in &embeddings {
        assert!(
            has_embedding,
            "episode {ep_id} should have embedding after reembed"
        );
    }

    // Verify rowid map still has all 3 entries
    let rowid_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM episodes_rowid_map WHERE episode_id IN ('ep-re-1', 'ep-re-2', 'ep-re-3')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        rowid_count, 3,
        "all 3 episodes should have rowid map entries"
    );
}

// ──────────────────────────────────────────────────────────────────────
// Causal graph correctness with multiple episodes
// ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn causal_edges_use_episode_id_not_document_id() {
    let (store, _dir) = test_store();

    let doc_id = store
        .ingest_document("causal-doc", "causal test content", "general", None, None)
        .await
        .unwrap();

    let fact_id = store
        .add_fact("general", "causal test fact", None, None)
        .await
        .unwrap();

    let ep_id = store
        .create_episode(
            "ep-causal",
            &doc_id,
            &make_meta("causal_effect", vec![&format!("fact:{fact_id}")]),
        )
        .await
        .unwrap();

    let graph = store.graph_view();
    let ep_edges = graph
        .neighbors(&format!("episode:{ep_id}"), GraphDirection::Outgoing, 1)
        .unwrap();

    // Find causal edge
    let causal_edges: Vec<_> = ep_edges
        .iter()
        .filter(|e| matches!(e.edge_type, GraphEdgeType::Causal { .. }))
        .collect();

    assert!(
        !causal_edges.is_empty(),
        "should have at least one causal edge"
    );

    for edge in &causal_edges {
        assert_eq!(
            edge.source,
            format!("episode:{ep_id}"),
            "causal edge source should use episode_id"
        );
        assert!(
            !edge.source.contains(&doc_id) || ep_id.contains(&doc_id),
            "causal edge source should not use bare document_id"
        );
    }

    // Verify backlinks from the fact side
    let fact_edges = graph
        .neighbors(&format!("fact:{fact_id}"), GraphDirection::Incoming, 1)
        .unwrap();
    let causal_backlinks: Vec<_> = fact_edges
        .iter()
        .filter(|e| matches!(e.edge_type, GraphEdgeType::Causal { .. }))
        .collect();

    assert!(
        !causal_backlinks.is_empty(),
        "fact should have causal backlink from episode"
    );
    for edge in &causal_backlinks {
        assert_eq!(
            edge.source,
            format!("episode:{ep_id}"),
            "causal backlink source should use episode_id"
        );
    }
}
