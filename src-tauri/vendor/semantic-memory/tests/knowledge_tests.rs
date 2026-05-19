#![allow(deprecated)]
#![allow(clippy::expect_used)]

use forge_memory_bridge::PROJECTION_IMPORT_BATCH_V1_SCHEMA;
use semantic_memory::{
    EpisodeMeta, EpisodeOutcome, MemoryConfig, MemoryStore, MockEmbedder, ProjectionQuery, Role,
    VerificationStatus,
};
use stack_ids::ScopeKey;
use tempfile::TempDir;

fn projection_batch_json(namespace: &str) -> String {
    let namespace = namespace.to_string();
    serde_json::json!({
        "source_envelope_id": "env-delete-ns",
        "schema_version": PROJECTION_IMPORT_BATCH_V1_SCHEMA,
        "export_schema_version": "export_envelope_v1",
        "content_digest": format!("digest-{namespace}"),
        "source_authority": "test",
        "scope_key": { "namespace": namespace },
        "source_exported_at": "2026-03-07T00:00:00Z",
        "transformed_at": "2026-03-07T00:00:01Z",
        "records": [
            {
                "kind": "claim_version",
                "claim_id": "claim-del-1",
                "claim_version_id": "claim-del-1-v1",
                "claim_state": "active",
                "projection_family": "forge_verification",
                "subject_entity_id": "ent-delete-1",
                "predicate": "has_type",
                "object_anchor": "function",
                "valid_from": "2026-01-01T00:00:00Z",
                "valid_to": null,
                "preferred_open": true,
                "freshness": "current",
                "contradiction_status": "none",
                "content": "Delete namespace claim",
                "confidence": 0.95
            },
            {
                "kind": "relation_version",
                "relation_version_id": "rel-del-1-v1",
                "subject_entity_id": "ent-delete-1",
                "predicate": "affects",
                "object_anchor": "ent-delete-target",
                "preferred_open": true,
                "source_confidence": 0.75,
                "projection_family": "forge_verification",
                "freshness": "current",
                "contradiction_status": "none"
            },
            {
                "kind": "entity_alias",
                "canonical_entity_id": "ent-delete-1",
                "alias_text": "delete-one",
                "alias_source": "manual",
                "confidence": 0.99,
                "merge_decision": { "automated": { "algorithm": "bridge_default" } },
                "scope": { "namespace": namespace },
                "review_state": "unreviewed",
                "is_human_confirmed": false,
                "is_human_confirmed_final": false
            },
            {
                "kind": "evidence_ref",
                "claim_id": "claim-del-1",
                "fetch_handle": "forge://evidence/delete-namespace",
                "source_authority": "test"
            },
            {
                "kind": "episode",
                "episode_id": "episode-delete-1",
                "document_id": "doc-delete",
                "cause_ids": ["claim-del-1"],
                "effect_type": "change",
                "outcome": "success",
                "confidence": 0.84
            }
        ]
    })
    .to_string()
}

fn episode_meta_for_delete_namespace() -> EpisodeMeta {
    EpisodeMeta {
        cause_ids: vec!["chunk-deleteme-doc".into()],
        effect_type: "test_effect".into(),
        outcome: EpisodeOutcome::Confirmed,
        confidence: 0.91,
        verification_status: VerificationStatus::Verified {
            method: "manual".into(),
            at: "2026-03-07T00:00:00Z".into(),
        },
        experiment_id: None,
    }
}

fn namespaced_session_metadata(namespace: &str) -> serde_json::Value {
    serde_json::json!({
        "namespace": namespace,
        "scope_namespace": namespace
    })
}

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
async fn add_fact_and_get() {
    let (store, _tmp) = test_store();
    let fact_id = store
        .add_fact("general", "The sky is blue", None, None)
        .await
        .unwrap();

    let fact = store
        .get_fact(&fact_id)
        .await
        .unwrap()
        .expect("fact should exist");
    assert_eq!(fact.content, "The sky is blue");
    assert_eq!(fact.namespace, "general");
}

#[tokio::test]
async fn add_fact_with_source_and_metadata() {
    let (store, _tmp) = test_store();
    let metadata = serde_json::json!({"confidence": 0.9});
    let fact_id = store
        .add_fact(
            "user",
            "Josh likes Rust",
            Some("conversation:abc"),
            Some(metadata),
        )
        .await
        .unwrap();

    let fact = store.get_fact(&fact_id).await.unwrap().unwrap();
    assert_eq!(fact.namespace, "user");
    assert_eq!(fact.source.as_deref(), Some("conversation:abc"));
    assert!(fact.metadata.is_some());
}

#[tokio::test]
async fn add_fact_with_embedding_sync() {
    let (store, _tmp) = test_store();
    let embedding = vec![0.1f32; 768];
    let fact_id = store
        .add_fact_with_embedding("test", "Pre-embedded fact", &embedding, None, None)
        .await
        .unwrap();

    let fact = store.get_fact(&fact_id).await.unwrap().unwrap();
    assert_eq!(fact.content, "Pre-embedded fact");
}

#[tokio::test]
async fn fts_finds_inserted_fact() {
    let (store, _tmp) = test_store();
    store
        .add_fact("general", "Rust programming language", None, None)
        .await
        .unwrap();

    let results = store
        .search_fts_only("Rust programming", None, None, None)
        .await
        .unwrap();
    assert!(!results.is_empty(), "FTS should find the fact");
    assert!(results[0].content.contains("Rust"));
}

#[tokio::test]
async fn update_fact_fts_reflects_new_content() {
    let (store, _tmp) = test_store();
    let fact_id = store
        .add_fact("general", "Old content about cats", None, None)
        .await
        .unwrap();

    // Should find old content
    let results = store
        .search_fts_only("cats", None, None, None)
        .await
        .unwrap();
    assert!(!results.is_empty());

    // Update
    store
        .update_fact(&fact_id, "New content about dogs")
        .await
        .unwrap();

    // Should find new content
    let results = store
        .search_fts_only("dogs", None, None, None)
        .await
        .unwrap();
    assert!(!results.is_empty());

    // Should NOT find old content
    let results = store
        .search_fts_only("cats", None, None, None)
        .await
        .unwrap();
    assert!(
        results.is_empty(),
        "FTS should not find old content after update"
    );
}

#[tokio::test]
async fn delete_fact_removes_from_fts() {
    let (store, _tmp) = test_store();
    let fact_id = store
        .add_fact("general", "Temporary fact about whales", None, None)
        .await
        .unwrap();

    let results = store
        .search_fts_only("whales", None, None, None)
        .await
        .unwrap();
    assert!(!results.is_empty());

    store.delete_fact(&fact_id).await.unwrap();

    let results = store
        .search_fts_only("whales", None, None, None)
        .await
        .unwrap();
    assert!(results.is_empty(), "FTS should return nothing after delete");
}

#[tokio::test]
async fn bulk_insert_delete_fts_consistency() {
    let (store, _tmp) = test_store();

    // Insert 20 facts
    let mut ids = Vec::new();
    for i in 0..20 {
        let id = store
            .add_fact("bulk", &format!("Bulk fact number {}", i), None, None)
            .await
            .unwrap();
        ids.push(id);
    }

    // Delete first 10
    for id in &ids[..10] {
        store.delete_fact(id).await.unwrap();
    }

    // FTS should only find 10
    let results = store
        .search_fts_only("Bulk fact", Some(30), None, None)
        .await
        .unwrap();
    assert_eq!(results.len(), 10, "Should have exactly 10 remaining facts");
}

#[tokio::test]
async fn namespace_filtering_on_list_facts() {
    let (store, _tmp) = test_store();
    store
        .add_fact("ns_a", "Fact in namespace A", None, None)
        .await
        .unwrap();
    store
        .add_fact("ns_b", "Fact in namespace B", None, None)
        .await
        .unwrap();
    store
        .add_fact("ns_a", "Another fact in namespace A", None, None)
        .await
        .unwrap();

    let facts_a = store.list_facts("ns_a", 100, 0).await.unwrap();
    assert_eq!(facts_a.len(), 2);

    let facts_b = store.list_facts("ns_b", 100, 0).await.unwrap();
    assert_eq!(facts_b.len(), 1);
}

#[tokio::test]
async fn delete_namespace() {
    let (store, _tmp) = test_store();
    let ns = "deleteme";
    store
        .add_fact(ns, "Fact 1 ghost target", None, None)
        .await
        .unwrap();
    store
        .add_fact(ns, "Fact 2 ghost target", None, None)
        .await
        .unwrap();
    store
        .add_fact("keepme", "Fact 3 survivor", None, None)
        .await
        .unwrap();

    let doc_id = store
        .ingest_document(
            "DeleteMe Doc",
            "Ghost content for deletion test",
            ns,
            None,
            None,
        )
        .await
        .unwrap();

    let _episode_id = store
        .ingest_episode(&doc_id, &episode_meta_for_delete_namespace())
        .await
        .unwrap();

    let session_id = store
        .create_session_with_metadata("chat", Some(namespaced_session_metadata(ns)))
        .await
        .unwrap();
    store
        .add_message(
            &session_id,
            Role::User,
            "Session-scoped message should be removed",
            None,
            None,
        )
        .await
        .unwrap();

    let projection_batch = projection_batch_json(ns);
    let projection_result = store
        .import_projection_batch_json_compat(&projection_batch)
        .await
        .unwrap();
    assert_eq!(projection_result.status, "complete");

    let doc_results = store.list_documents(ns, 10, 0).await.unwrap();
    assert_eq!(doc_results.len(), 1);

    let facts = store
        .search_fts_only("DeleteMe", Some(10), Some(&[ns]), None)
        .await
        .unwrap();
    assert!(
        !facts.is_empty(),
        "ingested document content should be queryable before deletion"
    );

    let claims = store
        .query_claim_versions(ProjectionQuery::new(ScopeKey::namespace_only(ns)))
        .await
        .unwrap();
    assert!(
        !claims.is_empty(),
        "projection claims should exist before deletion"
    );

    let relations = store
        .query_relation_versions(ProjectionQuery::new(ScopeKey::namespace_only(ns)))
        .await
        .unwrap();
    assert!(
        !relations.is_empty(),
        "projection relations should exist before deletion"
    );

    let episodes = store
        .query_episodes(ProjectionQuery::new(ScopeKey::namespace_only(ns)))
        .await
        .unwrap();
    assert!(
        !episodes.is_empty(),
        "projection episodes should exist before deletion"
    );

    let aliases = store
        .query_entity_aliases(ProjectionQuery::new(ScopeKey::namespace_only(ns)))
        .await
        .unwrap();
    assert!(
        !aliases.is_empty(),
        "projection aliases should exist before deletion"
    );

    let evidence = store
        .query_evidence_refs(ProjectionQuery::new(ScopeKey::namespace_only(ns)))
        .await
        .unwrap();
    assert!(
        !evidence.is_empty(),
        "projection evidence refs should exist before deletion"
    );

    let projection_imports = store.query_projection_imports(Some(ns), 10).await.unwrap();
    assert!(
        !projection_imports.is_empty(),
        "projection imports should exist before deletion"
    );

    let projection_failures = store
        .query_projection_import_failures(Some(ns), 10)
        .await
        .unwrap();
    assert!(
        projection_failures.is_empty(),
        "projection failures should be empty before deletion"
    );

    let sessions_before = store.list_sessions(20, 0).await.unwrap();
    assert!(
        sessions_before
            .iter()
            .any(|session| session.id == session_id),
        "namespaced session should exist before namespace deletion"
    );

    let report = store.delete_namespace("deleteme").await.unwrap();
    assert_eq!(report.facts, 2);
    assert_eq!(report.documents, 1);
    assert!(report.chunks > 0);
    assert_eq!(report.sessions, 1);
    assert_eq!(report.messages, 1);
    assert_eq!(report.episodes, 1);
    assert!(report.projection_rows > 0);

    let remaining = store.list_facts(ns, 100, 0).await.unwrap();
    assert!(remaining.is_empty());

    let kept = store.list_facts("keepme", 100, 0).await.unwrap();
    assert_eq!(kept.len(), 1);

    let deleted_one = store
        .search_fts_only("ghost target", Some(10), None, None)
        .await
        .unwrap();
    assert!(
        deleted_one.is_empty(),
        "deleted namespace facts must not leave ghost FTS rows"
    );

    let survivor = store
        .search_fts_only("survivor", Some(10), None, None)
        .await
        .unwrap();
    assert_eq!(
        survivor.len(),
        1,
        "surviving namespace fact must remain searchable"
    );

    let namespace_fts = store
        .search_fts_only("DeleteMe", Some(10), Some(&[ns]), None)
        .await
        .unwrap();
    assert!(
        namespace_fts.is_empty(),
        "deleted namespace documents and chunks should not leak FTS rows"
    );

    let remaining_docs = store.list_documents(ns, 10, 0).await.unwrap();
    assert!(
        remaining_docs.is_empty(),
        "deleted namespace docs must be removed"
    );

    let claims_after = store
        .query_claim_versions(ProjectionQuery::new(ScopeKey::namespace_only(ns)))
        .await
        .unwrap();
    assert!(
        claims_after.is_empty(),
        "projection claim rows must be removed for deleted namespace"
    );
    let relations_after = store
        .query_relation_versions(ProjectionQuery::new(ScopeKey::namespace_only(ns)))
        .await
        .unwrap();
    assert!(
        relations_after.is_empty(),
        "projection relation rows must be removed for deleted namespace"
    );
    let episodes_after = store
        .query_episodes(ProjectionQuery::new(ScopeKey::namespace_only(ns)))
        .await
        .unwrap();
    assert!(
        episodes_after.is_empty(),
        "projection episode rows must be removed for deleted namespace"
    );
    let aliases_after = store
        .query_entity_aliases(ProjectionQuery::new(ScopeKey::namespace_only(ns)))
        .await
        .unwrap();
    assert!(
        aliases_after.is_empty(),
        "projection aliases must be removed for deleted namespace"
    );
    let evidence_after = store
        .query_evidence_refs(ProjectionQuery::new(ScopeKey::namespace_only(ns)))
        .await
        .unwrap();
    assert!(
        evidence_after.is_empty(),
        "projection evidence refs must be removed for deleted namespace"
    );
    let import_log_after = store.query_projection_imports(Some(ns), 10).await.unwrap();
    assert!(
        import_log_after.is_empty(),
        "projection import logs must be removed for deleted namespace"
    );
    let failures_after = store
        .query_projection_import_failures(Some(ns), 10)
        .await
        .unwrap();
    assert!(
        failures_after.is_empty(),
        "projection import failures remain empty after deletion"
    );

    let sessions_after = store.list_sessions(20, 0).await.unwrap();
    assert!(
        !sessions_after
            .iter()
            .any(|session| session.id == session_id),
        "namespaced session and its messages should be removed when namespace is deleted"
    );
}

#[tokio::test]
async fn embedding_blob_roundtrip() {
    let (store, _tmp) = test_store();
    let original: Vec<f32> = (0..768).map(|i| (i as f32) * 0.001).collect();
    let fact_id = store
        .add_fact_with_embedding("test", "Roundtrip test", &original, None, None)
        .await
        .unwrap();

    // Verify the fact exists and content matches
    let fact = store.get_fact(&fact_id).await.unwrap().unwrap();
    assert_eq!(fact.content, "Roundtrip test");

    // Verify embedding roundtrip via embed helper
    let stored = store
        .get_fact_embedding(&fact_id)
        .await
        .unwrap()
        .expect("embedding should exist");
    assert_eq!(stored.len(), original.len());
    for (a, b) in stored.iter().zip(original.iter()) {
        assert!(
            (a - b).abs() < 1e-6,
            "Embedding values should match: {} vs {}",
            a,
            b
        );
    }
}

#[tokio::test]
async fn get_nonexistent_fact_returns_none() {
    let (store, _tmp) = test_store();
    let result = store.get_fact("nonexistent-uuid").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn delete_nonexistent_fact_returns_error() {
    let (store, _tmp) = test_store();
    let result = store.delete_fact("nonexistent-uuid").await;
    assert!(result.is_err());
}
