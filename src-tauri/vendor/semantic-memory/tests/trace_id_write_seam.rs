#![allow(clippy::expect_used)]

use semantic_memory::{MemoryConfig, MemoryStore, MockEmbedder, Role, VerifyMode};
use stack_ids::TraceCtx;
use tempfile::TempDir;

fn open_store(dir: &TempDir) -> MemoryStore {
    let config = MemoryConfig {
        base_dir: dir.path().to_path_buf(),
        ..Default::default()
    };
    let embedder = Box::new(MockEmbedder::new(config.embedding.dimensions));
    MemoryStore::open_with_embedder(config, embedder).expect("open store")
}

#[tokio::test]
async fn write_variants_persist_trace_id() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);
    let trace_ctx = TraceCtx::from_trace_id("trace-semantic-001");

    let fact_id = store
        .add_fact_with_trace("general", "traceable fact", None, None, Some(&trace_ctx))
        .await
        .unwrap();
    let fact = store.get_fact(&fact_id).await.unwrap().unwrap();
    assert_eq!(fact.metadata.unwrap()["trace_id"], "trace-semantic-001");

    let session_id = store.create_session("trace-test").await.unwrap();
    let msg_id = store
        .add_message_with_trace(
            &session_id,
            Role::User,
            "traceable message",
            None,
            None,
            Some(&trace_ctx),
        )
        .await
        .unwrap();

    let messages = store.get_recent_messages(&session_id, 10).await.unwrap();
    let message = messages.into_iter().find(|m| m.id == msg_id).unwrap();
    assert_eq!(message.metadata.unwrap()["trace_id"], "trace-semantic-001");

    store
        .ingest_document_with_trace(
            "Traceable Doc",
            "document body",
            "docs",
            None,
            None,
            Some(&trace_ctx),
        )
        .await
        .unwrap();
    let documents = store.list_documents("docs", 10, 0).await.unwrap();
    assert_eq!(
        documents[0].metadata.as_ref().unwrap()["trace_id"],
        "trace-semantic-001"
    );

    store
        .ingest_episode_with_trace(
            &documents[0].id,
            &semantic_memory::EpisodeMeta {
                cause_ids: vec![fact_id.clone()],
                effect_type: "traceable_incident".to_string(),
                outcome: semantic_memory::EpisodeOutcome::Pending,
                confidence: 0.7,
                verification_status: semantic_memory::VerificationStatus::Unverified,
                experiment_id: None,
            },
            Some(&trace_ctx),
        )
        .await
        .unwrap();

    let conn = rusqlite::Connection::open(dir.path().join("memory.db")).unwrap();
    let stored_trace_id: Option<String> = conn
        .query_row(
            "SELECT trace_id FROM episodes WHERE document_id = ?1",
            rusqlite::params![&documents[0].id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_trace_id.as_deref(), Some("trace-semantic-001"));

    let report = store.verify_integrity(VerifyMode::Full).await.unwrap();
    assert!(report.ok, "integrity should pass after trace-aware writes");
}
