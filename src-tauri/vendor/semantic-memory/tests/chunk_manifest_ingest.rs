use semantic_memory::{
    ChunkManifestEntry, ChunkManifestIngestOptions, MemoryConfig, MemoryError, MemoryStore,
    MockEmbedder, SearchSource,
};
use tempfile::TempDir;

fn test_store() -> (MemoryStore, TempDir) {
    let tmp = TempDir::new().unwrap();
    let config = MemoryConfig {
        base_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let store = MemoryStore::open_with_embedder(config, Box::new(MockEmbedder::new(768))).unwrap();
    (store, tmp)
}

fn options(namespace: &str) -> ChunkManifestIngestOptions {
    ChunkManifestIngestOptions {
        title: "Gloss source".to_string(),
        namespace: namespace.to_string(),
        source_path: Some("gloss://source/source-a".to_string()),
        metadata: Some(serde_json::json!({
            "external_document_id": "source-a",
            "scope_domain": "gloss"
        })),
    }
}

fn entry(id: &str, content: &str) -> ChunkManifestEntry {
    ChunkManifestEntry {
        external_chunk_id: id.to_string(),
        content: content.to_string(),
        token_count_estimate: Some(8),
        content_digest: Some(format!("digest-{id}")),
        metadata: None,
    }
}

#[tokio::test]
async fn ingest_chunk_manifest_returns_exact_mapping() {
    let (store, _tmp) = test_store();

    let result = store
        .ingest_chunk_manifest(
            options("notebook-a"),
            vec![
                entry("chunk-a", "alpha chunk about exact manifest mapping"),
                entry("chunk-b", "beta chunk about preserved backpointers"),
            ],
        )
        .await
        .unwrap();

    assert_eq!(result.namespace, "notebook-a");
    assert!(!result.sm_document_id.is_empty());
    assert!(result.receipt_id.starts_with("chunk-manifest:"));
    assert_eq!(result.chunks.len(), 2);
    assert_eq!(result.chunks[0].external_chunk_id, "chunk-a");
    assert_eq!(result.chunks[1].external_chunk_id, "chunk-b");
    assert_eq!(result.chunks[0].chunk_index, 0);
    assert_eq!(result.chunks[1].chunk_index, 1);
    assert_eq!(result.chunks[0].sm_document_id, result.sm_document_id);
    assert_eq!(result.chunks[1].sm_document_id, result.sm_document_id);
    assert_ne!(result.chunks[0].sm_chunk_id, result.chunks[1].sm_chunk_id);
    assert_eq!(
        result.chunks[0].content_digest.as_deref(),
        Some("digest-chunk-a")
    );

    let docs = store.list_documents("notebook-a", 10, 0).await.unwrap();
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].id, result.sm_document_id);
    assert_eq!(docs[0].chunk_count, 2);
}

#[tokio::test]
async fn search_chunk_result_uses_returned_sm_chunk_id() {
    let (store, _tmp) = test_store();

    let result = store
        .ingest_chunk_manifest(
            options("notebook-a"),
            vec![entry(
                "external-unique",
                "needle phrase only this manifest chunk should contain",
            )],
        )
        .await
        .unwrap();
    let expected_sm_chunk_id = result.chunks[0].sm_chunk_id.clone();

    let search = store
        .search_fts_only("needle phrase", Some(10), Some(&["notebook-a"]), None)
        .await
        .unwrap();
    assert!(search.iter().any(|hit| {
        matches!(
            &hit.source,
            SearchSource::Chunk { chunk_id, .. } if chunk_id == &expected_sm_chunk_id
        )
    }));
}

#[tokio::test]
async fn duplicate_external_chunk_id_rejected_before_insert() {
    let (store, _tmp) = test_store();

    let err = store
        .ingest_chunk_manifest(
            options("notebook-a"),
            vec![entry("dup", "first chunk"), entry("dup", "second chunk")],
        )
        .await
        .unwrap_err();

    assert!(err.to_string().contains("duplicate external_chunk_id"));
    let docs = store.list_documents("notebook-a", 10, 0).await.unwrap();
    assert!(docs.is_empty());
}

#[tokio::test]
async fn empty_chunk_rejected_before_insert() {
    let (store, _tmp) = test_store();

    let err = store
        .ingest_chunk_manifest(
            options("notebook-a"),
            vec![ChunkManifestEntry {
                external_chunk_id: "empty".to_string(),
                content: String::new(),
                token_count_estimate: None,
                content_digest: Some("digest-empty".to_string()),
                metadata: None,
            }],
        )
        .await
        .unwrap_err();

    assert!(err.to_string().contains("content must not be empty"));
    let docs = store.list_documents("notebook-a", 10, 0).await.unwrap();
    assert!(docs.is_empty());
}

#[tokio::test]
async fn namespace_isolation_preserved() {
    let (store, _tmp) = test_store();

    store
        .ingest_chunk_manifest(
            options("notebook-a"),
            vec![entry("chunk-a", "same phrase in notebook a")],
        )
        .await
        .unwrap();
    store
        .ingest_chunk_manifest(
            options("notebook-b"),
            vec![entry("chunk-b", "same phrase in notebook b")],
        )
        .await
        .unwrap();

    let a_docs = store.list_documents("notebook-a", 10, 0).await.unwrap();
    let b_docs = store.list_documents("notebook-b", 10, 0).await.unwrap();
    assert_eq!(a_docs.len(), 1);
    assert_eq!(b_docs.len(), 1);
    assert_ne!(a_docs[0].id, b_docs[0].id);
}

#[tokio::test]
async fn too_many_manifest_chunks_rejected() {
    let tmp = TempDir::new().unwrap();
    let mut config = MemoryConfig {
        base_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    config.limits.max_chunks_per_document = 1;
    let store = MemoryStore::open_with_embedder(config, Box::new(MockEmbedder::new(768))).unwrap();

    let err = store
        .ingest_chunk_manifest(
            options("notebook-a"),
            vec![
                entry("chunk-a", "first chunk"),
                entry("chunk-b", "second chunk"),
            ],
        )
        .await
        .unwrap_err();

    assert!(matches!(
        err,
        MemoryError::ContentTooLarge { size: 2, limit: 1 }
    ));
}
