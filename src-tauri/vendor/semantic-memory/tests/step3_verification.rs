//! Step 3 verification tests: FTS, integrity, and limit enforcement.
#![allow(clippy::expect_used)]

use semantic_memory::{
    IntegrityReport, MemoryConfig, MemoryLimits, MemoryStore, MockEmbedder, ReconcileAction, Role,
    VerifyMode,
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

// ─── FTS Tests ─────────────────────────────────────────────────

#[tokio::test]
async fn test_fts_message_indexing() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    let session_id = store.create_session("test").await.unwrap();
    store
        .add_message_fts(
            &session_id,
            Role::User,
            "The quick brown fox jumps",
            None,
            None,
        )
        .await
        .unwrap();
    store
        .add_message_fts(
            &session_id,
            Role::Assistant,
            "The lazy dog sleeps",
            None,
            None,
        )
        .await
        .unwrap();

    // Verify messages were stored
    let messages = store.get_recent_messages(&session_id, 10).await.unwrap();
    assert_eq!(messages.len(), 2);

    // Verify FTS content is searchable
    let report = store.verify_integrity(VerifyMode::Full).await.unwrap();
    assert_eq!(report.message_count, 2);
}

#[tokio::test]
async fn test_fts_fact_indexing() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    store
        .add_fact("general", "Rust was first released in 2015", None, None)
        .await
        .unwrap();
    store
        .add_fact("general", "Go was first released in 2009", None, None)
        .await
        .unwrap();

    let report = store.verify_integrity(VerifyMode::Full).await.unwrap();
    assert_eq!(report.fact_count, 2);
    assert!(report.ok, "integrity should pass: {:?}", report.issues);
}

// ─── Integrity Tests ───────────────────────────────────────────

#[tokio::test]
async fn test_integrity_quick_check() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    let report: IntegrityReport = store.verify_integrity(VerifyMode::Quick).await.unwrap();
    assert!(report.ok);
    assert_eq!(report.fact_count, 0);
    assert_eq!(report.chunk_count, 0);
    assert_eq!(report.message_count, 0);
}

#[tokio::test]
async fn test_integrity_full_check() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    store
        .add_fact("test", "some fact content", None, None)
        .await
        .unwrap();

    let report = store.verify_integrity(VerifyMode::Full).await.unwrap();
    assert!(report.ok, "integrity should pass: {:?}", report.issues);
    assert_eq!(report.fact_count, 1);
    assert!(report.schema_version > 0);
}

#[tokio::test]
async fn test_integrity_reconcile_report_only() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    store
        .add_fact("test", "reconcile test", None, None)
        .await
        .unwrap();

    let report = store.reconcile(ReconcileAction::ReportOnly).await.unwrap();
    assert!(report.ok);
    assert_eq!(report.fact_count, 1);
}

#[tokio::test]
async fn test_integrity_reconcile_rebuild_fts() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    store
        .add_fact("test", "rebuild fts test", None, None)
        .await
        .unwrap();

    let report = store.reconcile(ReconcileAction::RebuildFts).await.unwrap();
    assert!(
        report.ok,
        "integrity should pass after FTS rebuild: {:?}",
        report.issues
    );
    assert_eq!(report.fact_count, 1);
}

// ─── Limit Enforcement Tests ──────────────────────────────────

#[tokio::test]
async fn test_limit_content_too_large_fact() {
    let dir = TempDir::new().unwrap();
    let mut config = test_config(&dir);
    config.limits.max_content_bytes = 100; // very small limit
    let embedder = Box::new(MockEmbedder::new(config.embedding.dimensions));
    let store = MemoryStore::open_with_embedder(config, embedder).unwrap();

    let big_content = "x".repeat(200);
    let result = store.add_fact("test", &big_content, None, None).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), "content_too_large");
}

#[tokio::test]
async fn test_limit_content_too_large_message() {
    let dir = TempDir::new().unwrap();
    let mut config = test_config(&dir);
    config.limits.max_content_bytes = 50;
    let embedder = Box::new(MockEmbedder::new(config.embedding.dimensions));
    let store = MemoryStore::open_with_embedder(config, embedder).unwrap();

    let session_id = store.create_session("test").await.unwrap();
    let big_content = "y".repeat(100);
    let result = store
        .add_message(&session_id, Role::User, &big_content, None, None)
        .await;
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind(), "content_too_large");
}

#[tokio::test]
async fn test_limit_content_too_large_message_fts() {
    let dir = TempDir::new().unwrap();
    let mut config = test_config(&dir);
    config.limits.max_content_bytes = 32;
    let embedder = Box::new(MockEmbedder::new(config.embedding.dimensions));
    let store = MemoryStore::open_with_embedder(config, embedder).unwrap();

    let session_id = store.create_session("test").await.unwrap();
    let big_content = "z".repeat(64);
    let result = store
        .add_message_fts(&session_id, Role::User, &big_content, None, None)
        .await;
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind(), "content_too_large");
}

#[tokio::test]
async fn test_limit_content_too_large_fact_with_embedding() {
    let dir = TempDir::new().unwrap();
    let mut config = test_config(&dir);
    config.limits.max_content_bytes = 16;
    let embedder = Box::new(MockEmbedder::new(config.embedding.dimensions));
    let store = MemoryStore::open_with_embedder(config, embedder).unwrap();

    let embedding = vec![0.1; 768];
    let result = store
        .add_fact_with_embedding("test", "this string is too large", &embedding, None, None)
        .await;
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind(), "content_too_large");
}

#[tokio::test]
async fn test_limit_content_too_large_document() {
    let dir = TempDir::new().unwrap();
    let mut config = test_config(&dir);
    config.limits.max_content_bytes = 64;
    let embedder = Box::new(MockEmbedder::new(config.embedding.dimensions));
    let store = MemoryStore::open_with_embedder(config, embedder).unwrap();

    let result = store
        .ingest_document("Too Big", &"a".repeat(128), "docs", None, None)
        .await;
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind(), "content_too_large");
}

#[tokio::test]
async fn test_limit_namespace_full() {
    let dir = TempDir::new().unwrap();
    let mut config = test_config(&dir);
    config.limits.max_facts_per_namespace = 2; // very small limit
    let embedder = Box::new(MockEmbedder::new(config.embedding.dimensions));
    let store = MemoryStore::open_with_embedder(config, embedder).unwrap();

    store.add_fact("ns1", "fact one", None, None).await.unwrap();
    store.add_fact("ns1", "fact two", None, None).await.unwrap();

    // Third fact should fail
    let result = store.add_fact("ns1", "fact three", None, None).await;
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind(), "namespace_full");

    // Different namespace should still work
    let result = store.add_fact("ns2", "fact one", None, None).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_limit_memory_limits_validated() {
    // Concurrency hard-capped at 32
    let limits = MemoryLimits {
        max_embedding_concurrency: 100,
        ..Default::default()
    }
    .validated();
    assert_eq!(limits.max_embedding_concurrency, 32);

    // Zero concurrency clamped to 1
    let limits = MemoryLimits {
        max_embedding_concurrency: 0,
        ..Default::default()
    }
    .validated();
    assert_eq!(limits.max_embedding_concurrency, 1);
}

// ─── PoolConfig Tests ──────────────────────────────────────────

#[tokio::test]
async fn test_pool_config_defaults_work() {
    let dir = TempDir::new().unwrap();
    let config = test_config(&dir);
    assert_eq!(config.pool.busy_timeout_ms, 5000);
    assert!(config.pool.enable_wal);

    let embedder = Box::new(MockEmbedder::new(config.embedding.dimensions));
    let store = MemoryStore::open_with_embedder(config, embedder);
    assert!(store.is_ok());
}

// ─── Schema Version Tests ──────────────────────────────────────

#[tokio::test]
async fn test_schema_version_set() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir);

    let report = store.verify_integrity(VerifyMode::Quick).await.unwrap();
    assert!(report.schema_version > 0, "schema_version should be set");
}
