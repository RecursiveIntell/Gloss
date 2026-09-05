//! Actual registry runtime plus the actual Gloss receipt acceptance owner.
//! MockEmbedder supplies deterministic disposable fixtures, not model quality evidence.
pub use gloss_native_contract_tests::db;
pub use gloss_native_contract_tests::error;
pub mod ingestion {
    pub use gloss_native_contract_tests::embedding_contract;
}
#[path = "../../../src-tauri/src/memory/ollama_embedder.rs"]
pub mod ollama_embedder;
#[path = "../../../src-tauri/src/memory/turbo_quant_proof.rs"]
pub mod turbo_quant_proof;

#[cfg(test)]
mod tests {
    use super::db::notebook_db::SemanticMemoryProjectionStatus;
    use super::turbo_quant_proof::{has_fresh_turbo_quant_proof, projection_artifact_proof};
    use semantic_memory::{
        ChunkManifestEntry, ChunkManifestIngestOptions, DerivedVectorBackendPolicy, MemoryConfig,
        MemoryStore, MockEmbedder, ReceiptMode, SearchContext, SearchSourceType,
    };
    use serde_json::Value;
    use tempfile::TempDir;

    fn config(directory: &TempDir, dimensions: usize) -> MemoryConfig {
        let mut config = MemoryConfig {
            base_dir: directory.path().to_owned(),
            ..Default::default()
        };
        config.embedding.dimensions = dimensions;
        config.search.derived_vector_backend = DerivedVectorBackendPolicy::TurboQuantCandidateOnly;
        config.search.turbo_quant_require_exact_rerank = true;
        config
    }

    fn open(directory: &TempDir) -> MemoryStore {
        open_with_dimensions(directory, 64)
    }

    fn open_with_dimensions(directory: &TempDir, dimensions: usize) -> MemoryStore {
        MemoryStore::open_with_embedder(
            config(directory, dimensions),
            Box::new(MockEmbedder::new(dimensions)),
        )
        .unwrap()
    }

    async fn fixture(store: &MemoryStore, namespace: &str) {
        store
            .ingest_chunk_manifest(
                ChunkManifestIngestOptions {
                    title: format!("{namespace} fixture"),
                    namespace: namespace.to_string(),
                    source_path: None,
                    metadata: None,
                },
                (0..8)
                    .map(|index| ChunkManifestEntry {
                        external_chunk_id: format!("{namespace}-{index}"),
                        content: format!(
                            "{namespace} deterministic notebook recovery vector fixture {index}"
                        ),
                        token_count_estimate: None,
                        content_digest: None,
                        metadata: None,
                    })
                    .collect(),
            )
            .await
            .unwrap();
    }

    async fn probe(store: &MemoryStore) -> Value {
        let mut context = SearchContext::default_now();
        context.receipt_mode = ReceiptMode::ReturnReceipt;
        let response = store
            .search_vector_only_with_context(
                "alpha deterministic notebook recovery vector fixture 0",
                Some(8),
                Some(&["alpha"]),
                Some(&[SearchSourceType::Chunks]),
                context,
            )
            .await
            .unwrap();
        assert!(!response.results.is_empty());
        assert!(response
            .results
            .iter()
            .all(|result| result.content.starts_with("alpha ")));
        serde_json::to_value(response.receipt.unwrap()).unwrap()
    }

    fn status(source: &str, receipt: &Value) -> SemanticMemoryProjectionStatus {
        SemanticMemoryProjectionStatus {
            notebook_id: "alpha".into(),
            source_id: source.into(),
            status: "synced".into(),
            chunk_count: 8,
            projected_chunk_count: 8,
            healthy_link_count: 8,
            degraded_link_count: 0,
            last_receipt_id: None,
            last_error: None,
            updated_at: String::new(),
            artifact_generation_id: receipt["artifact_generation_id"]
                .as_str()
                .map(str::to_owned),
            vector_artifact_manifest_digest: receipt["vector_artifact_manifest_digest"]
                .as_str()
                .map(str::to_owned),
        }
    }

    #[test]
    fn embedding_identity_apply_invalidates_prior_proof_but_tuning_preserves_it() {
        use super::db::notebook_db::{
            EmbeddingIndexMetadata, NotebookDb, SEMANTIC_MEMORY_INDEX_ID,
        };
        use gloss_native_contract_tests::settings_contract::{
            invalidate_existing_embedding_indexes, save_embedding_settings, EmbeddingSettings,
        };
        let directory = TempDir::new().unwrap();
        let app = super::db::app_db::AppDb::open(&directory.path().join("app.db")).unwrap();
        let db = NotebookDb::open(&directory.path().join("notebook.db")).unwrap();
        let mut config = EmbeddingSettings {
            provider: "ollama".into(),
            url: "http://localhost:11434".into(),
            model: "before".into(),
            timeout_secs: 60,
            download_consent: false,
            search_timeout_ms: 8000,
            chunk_target_tokens: 1100,
        };
        save_embedding_settings(&app, &config).unwrap();
        db.upsert_embedding_index_metadata(&EmbeddingIndexMetadata::ready(
            SEMANTIC_MEMORY_INDEX_ID,
            "ollama",
            "before",
            Some("identity-before".into()),
            64,
        ))
        .unwrap();
        db.conn().execute_batch("INSERT INTO sources(id, source_type, title, status) VALUES ('a', 'text', 'retained', 'ready');
            INSERT INTO chunks(id, source_id, chunk_index, content) VALUES ('c', 'a', 0, 'canonical content');
            INSERT INTO semantic_memory_projection_status(notebook_id, source_id, status, chunk_count, projected_chunk_count, healthy_link_count, artifact_generation_id, vector_artifact_manifest_digest)
            VALUES ('notebook', 'a', 'synced', 1, 1, 1, 'generation', 'digest');").unwrap();
        // This synthetic complete receipt tests acceptance/invalidation only.
        let receipt = serde_json::json!({
            "candidate_backend": super::turbo_quant_proof::TURBO_QUANT_BACKEND,
            "exact_rerank": true, "exact_rerank_count": 1,
            "approximate_scanned_count": 1, "approximate_returned_count": 1,
            "artifact_corruption_count": 0, "vector_artifact_missing_count": 0,
            "vector_artifact_stale_count": 0, "artifact_generation_id": "generation",
            "vector_artifact_manifest_digest": "digest", "fallback": null,
        });
        let proof = || {
            projection_artifact_proof(
                &[("a".into(), 1)],
                &db.list_semantic_memory_projection_statuses("notebook")
                    .unwrap(),
                Some(&receipt),
            )
        };
        assert!(proof().probe_matches);
        config.timeout_secs = 90;
        assert!(!save_embedding_settings(&app, &config).unwrap());
        assert!(proof().probe_matches);
        config.model = "after".into();
        assert!(save_embedding_settings(&app, &config).unwrap());
        invalidate_existing_embedding_indexes(&db).unwrap();
        assert!(!proof().probe_matches);
        assert!(proof().generation_id.is_none());
        assert_eq!(db.get_source("a").unwrap().title, "retained");
        assert_eq!(
            db.get_chunks_for_source("a").unwrap()[0].content,
            "canonical content"
        );
    }

    #[test]
    fn canonical_chunk_mutations_invalidate_notebook_artifact_proof_and_rebuild_restores_it() {
        use super::db::notebook_db::{Chunk, NotebookDb, SemanticMemoryProjectionStatusUpdate};
        let directory = TempDir::new().unwrap();
        let db = NotebookDb::open(&directory.path().join("notebook.db")).unwrap();
        for source_id in ["a", "b"] {
            db.conn().execute("INSERT INTO sources(id, source_type, title, status) VALUES (?1, 'text', ?1, 'ready')", [source_id]).unwrap();
            db.upsert_semantic_memory_projection_status(&SemanticMemoryProjectionStatusUpdate {
                notebook_id: "notebook".into(),
                source_id: source_id.into(),
                status: "synced".into(),
                chunk_count: 1,
                projected_chunk_count: 1,
                healthy_link_count: 1,
                degraded_link_count: 0,
                last_receipt_id: None,
                last_error: None,
                artifact_generation_id: Some("generation-before".into()),
                vector_artifact_manifest_digest: Some("digest-before".into()),
            })
            .unwrap();
        }
        let chunk = Chunk {
            id: "chunk-a".into(),
            source_id: "a".into(),
            chunk_index: 0,
            content: "canonical content".into(),
            token_count: None,
            start_offset: None,
            end_offset: None,
            metadata: None,
            embedding_id: None,
            embedding_model: None,
        };
        db.insert_chunks(&[chunk]).unwrap();
        let statuses = db
            .list_semantic_memory_projection_statuses("notebook")
            .unwrap();
        assert!(statuses
            .iter()
            .all(|status| status.artifact_generation_id.is_none()
                && status.status == "artifact_stale"));
        db.update_semantic_memory_projection_artifact(
            "notebook",
            Some("build-after"),
            Some("generation-after"),
            Some("digest-after"),
            None,
        )
        .unwrap();
        assert!(db
            .list_semantic_memory_projection_statuses("notebook")
            .unwrap()
            .iter()
            .all(|status| status.status == "synced"));
        db.conn().execute_batch("CREATE TRIGGER fail_source_delete BEFORE DELETE ON sources BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
        assert!(db
            .delete_source_with_projection_invalidation("notebook", "a")
            .is_err());
        assert!(db
            .list_semantic_memory_projection_statuses("notebook")
            .unwrap()
            .iter()
            .all(|status| status.artifact_generation_id.as_deref() == Some("generation-after")));
        db.conn()
            .execute_batch("DROP TRIGGER fail_source_delete;")
            .unwrap();
        db.delete_source_with_projection_invalidation("notebook", "a")
            .unwrap();
        assert!(db
            .list_semantic_memory_projection_statuses("notebook")
            .unwrap()
            .iter()
            .all(|status| status.artifact_generation_id.is_none()));
    }

    #[tokio::test]
    async fn actual_runtime_build_reopen_and_notebook_filter_supply_complete_proof() {
        let directory = TempDir::new().unwrap();
        let store = open_with_dimensions(&directory, 768);
        fixture(&store, "alpha").await;
        fixture(&store, "excluded").await;
        let build = store.rebuild_vector_artifacts().await.unwrap();
        assert_eq!(build.source_row_count, 16);
        let receipt = probe(&store).await;
        assert!(has_fresh_turbo_quant_proof(&receipt), "{receipt:#}");
        drop(store);
        let reopened = open_with_dimensions(&directory, 768);
        let resumed = probe(&reopened).await;
        assert!(has_fresh_turbo_quant_proof(&resumed), "{resumed:#}");
        assert_eq!(
            receipt["artifact_generation_id"],
            resumed["artifact_generation_id"]
        );
    }

    #[tokio::test]
    async fn actual_runtime_missing_stale_and_corrupt_fallbacks_never_prove_turbo_quant() {
        for fault in ["missing", "stale", "corrupt"] {
            let directory = TempDir::new().unwrap();
            let store = open(&directory);
            fixture(&store, "alpha").await;
            if fault != "missing" {
                store.rebuild_vector_artifacts().await.unwrap();
                let connection =
                    rusqlite::Connection::open(directory.path().join("memory.db")).unwrap();
                let sql = match fault {
                    "stale" => "UPDATE derived_vector_artifact_generations SET source_snapshot_digest = 'blake3:stale' WHERE status = 'active'",
                    _ => "UPDATE derived_vector_artifacts SET encoded = x'00'",
                };
                assert!(connection.execute(sql, []).unwrap() > 0);
            }
            let receipt = probe(&store).await;
            assert!(receipt["fallback"].is_string(), "{fault}: {receipt:#}");
            assert!(
                !has_fresh_turbo_quant_proof(&receipt),
                "{fault}: {receipt:#}"
            );
            store.rebuild_vector_artifacts().await.unwrap();
            assert!(has_fresh_turbo_quant_proof(&probe(&store).await));
        }
    }

    #[tokio::test]
    async fn actual_receipt_cannot_be_reused_for_another_generation_or_incomplete_sources() {
        let directory = TempDir::new().unwrap();
        let store = open(&directory);
        fixture(&store, "alpha").await;
        store.rebuild_vector_artifacts().await.unwrap();
        let receipt = probe(&store).await;
        let ids = vec![("source-a".into(), 8), ("source-b".into(), 8)];
        let statuses = vec![status("source-a", &receipt), status("source-b", &receipt)];
        assert!(projection_artifact_proof(&ids, &statuses, Some(&receipt)).probe_matches);
        let changed_chunk_count = vec![("source-a".into(), 9), ("source-b".into(), 8)];
        assert!(
            !projection_artifact_proof(&changed_chunk_count, &statuses, Some(&receipt))
                .probe_matches
        );
        assert!(!projection_artifact_proof(&ids, &statuses[..1], Some(&receipt)).probe_matches);
        let mut stale = statuses.clone();
        stale[1].status = "artifact_stale".into();
        assert_eq!(
            projection_artifact_proof(&ids, &stale, Some(&receipt)).stale_sources,
            1
        );
        let mut mixed = statuses.clone();
        mixed[1].artifact_generation_id = Some("other-generation".into());
        assert!(!projection_artifact_proof(&ids, &mixed, Some(&receipt)).probe_matches);
        let mut new_generation = statuses;
        for status in &mut new_generation {
            status.artifact_generation_id = Some("new-generation".into());
        }
        assert!(!projection_artifact_proof(&ids, &new_generation, Some(&receipt)).probe_matches);
        assert!(!projection_artifact_proof(&ids, &new_generation, None).probe_matches);
    }

    #[tokio::test]
    async fn absent_or_malformed_fields_do_not_turn_into_successful_proof() {
        let directory = TempDir::new().unwrap();
        let store = open(&directory);
        fixture(&store, "alpha").await;
        store.rebuild_vector_artifacts().await.unwrap();
        let receipt = probe(&store).await;
        for field in [
            "artifact_corruption_count",
            "vector_artifact_missing_count",
            "vector_artifact_stale_count",
            "exact_rerank_count",
            "approximate_scanned_count",
            "approximate_returned_count",
            "fallback",
        ] {
            let mut missing = receipt.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(!has_fresh_turbo_quant_proof(&missing), "missing {field}");
            let mut malformed = receipt.clone();
            malformed[field] = Value::String("0".into());
            assert!(
                !has_fresh_turbo_quant_proof(&malformed),
                "malformed {field}"
            );
        }
    }
}
