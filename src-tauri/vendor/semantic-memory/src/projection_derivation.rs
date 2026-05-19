use crate::{projection_storage, MemoryError, MemoryStore};

impl MemoryStore {
    /// Invalidate derivation edges matching a trigger mode, bounded by source artifact.
    ///
    /// Returns the number of edges invalidated. This enables bounded recomputation:
    /// only derived artifacts downstream of the specified source are affected.
    pub async fn invalidate_derivations(
        &self,
        source_kind: &str,
        source_id: &str,
        trigger_mode: &str,
        reason: &str,
    ) -> Result<usize, MemoryError> {
        let sk = source_kind.to_string();
        let si = source_id.to_string();
        let tm = trigger_mode.to_string();
        let r = reason.to_string();
        self.with_write_conn(move |conn| {
            projection_storage::invalidate_derivation_edges(conn, &sk, &si, &tm, &r)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryConfig, MockEmbedder};
    use forge_memory_bridge::PROJECTION_IMPORT_BATCH_V1_SCHEMA;
    use tempfile::TempDir;

    fn test_store() -> (MemoryStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let config = MemoryConfig {
            base_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        let store =
            MemoryStore::open_with_embedder(config, Box::new(MockEmbedder::new(768))).unwrap();
        (store, dir)
    }

    fn derivation_rich_batch() -> String {
        serde_json::json!({
            "source_envelope_id": "env-derivation",
            "schema_version": PROJECTION_IMPORT_BATCH_V1_SCHEMA,
            "export_schema_version": "export_envelope_v1",
            "content_digest": "digest-derivation",
            "source_authority": "forge",
            "scope_key": { "namespace": "test-ns" },
            "source_exported_at": "2026-03-07T00:00:00Z",
            "transformed_at": "2026-03-07T00:00:01Z",
            "records": [
                {
                    "kind": "claim_version",
                    "claim_id": "claim-1",
                    "claim_version_id": "claim-1-v1",
                    "scope_key": { "namespace": "test-ns" },
                    "claim_state": "active",
                    "projection_family": "forge_verification",
                    "subject_entity_id": "ent-1",
                    "predicate": "has_type",
                    "object_anchor": "function",
                    "valid_from": "2026-01-01T00:00:00Z",
                    "valid_to": null,
                    "preferred_open": true,
                    "source_envelope_id": "env-derivation",
                    "source_authority": "forge",
                    "freshness": "current",
                    "contradiction_status": "none",
                    "supersedes_claim_version_id": "claim-1-v0",
                    "content": "Entity ent-1 is a function",
                    "confidence": 0.95
                },
                {
                    "kind": "claim_version",
                    "claim_id": "claim-2",
                    "claim_version_id": "claim-2-v1",
                    "claim_state": "active",
                    "projection_family": "forge_verification",
                    "subject_entity_id": "ent-2",
                    "predicate": "depends_on",
                    "object_anchor": "ent-3",
                    "scope_key": { "namespace": "test-ns" },
                    "valid_from": "2026-01-01T00:00:00Z",
                    "valid_to": null,
                    "preferred_open": true,
                    "freshness": "current",
                    "contradiction_status": "none",
                    "content": "Entity ent-2 depends on ent-3",
                    "confidence": 0.9
                },
                {
                    "kind": "relation_version",
                    "relation_version_id": "rel-1-v1",
                    "scope_key": { "namespace": "test-ns" },
                    "subject_entity_id": "ent-1",
                    "predicate": "depends_on",
                    "object_anchor": "ent-2",
                    "preferred_open": true,
                    "claim_id": "claim-1",
                    "source_episode_id": "episode-1",
                    "supersedes_relation_version_id": "rel-1-v0",
                    "source_confidence": 0.84,
                    "projection_family": "forge_verification",
                    "freshness": "current",
                    "contradiction_status": "none",
                },
                {
                    "kind": "episode",
                    "episode_id": "episode-1",
                    "scope_key": { "namespace": "test-ns" },
                    "document_id": "doc-1",
                    "cause_ids": ["claim-1", "claim-2"],
                    "effect_type": "code_change",
                    "outcome": "success",
                    "confidence": 0.88
                },
                {
                    "kind": "entity_alias",
                    "canonical_entity_id": "ent-1",
                    "alias_text": "Entity One",
                    "alias_source": "forge_extraction",
                    "confidence": 0.9,
                    "merge_decision": { "automated": { "algorithm": "bridge_default" } },
                    "scope": { "namespace": "test-ns" },
                    "review_state": "unreviewed",
                    "is_human_confirmed": false,
                    "is_human_confirmed_final": false,
                    "superseded_by_entity_id": "ent-old",
                    "split_from_entity_id": "ent-split"
                },
                {
                    "kind": "evidence_ref",
                    "claim_id": "claim-1",
                    "claim_version_id": "claim-1-v1",
                    "fetch_handle": "forge://evidence/claim-1/v1",
                    "source_authority": "forge"
                },
                {
                    "kind": "evidence_ref",
                    "claim_id": "claim-2",
                    "fetch_handle": "forge://evidence/claim-2",
                    "source_authority": "forge"
                }
            ]
        })
        .to_string()
    }

    fn edge_exists(
        edges: &[projection_storage::DerivationEdgeRow],
        target_kind: &str,
        target_id: &str,
        derivation_type: &str,
        invalidation_mode: &str,
    ) -> bool {
        edges.iter().any(|edge| {
            edge.target_kind == target_kind
                && edge.target_id == target_id
                && edge.derivation_type == derivation_type
                && edge.invalidation_mode == invalidation_mode
        })
    }

    #[tokio::test]
    async fn import_projection_batch_inserts_broad_derivation_edges() {
        let (store, _dir) = test_store();

        store
            .import_projection_batch_json_compat(&derivation_rich_batch())
            .await
            .unwrap();

        let claim_1_edges = store
            .with_read_conn(|conn| {
                projection_storage::query_derivation_edges_by_source(conn, "claim", "claim-1")
            })
            .await
            .unwrap();

        assert!(edge_exists(
            &claim_1_edges,
            "claim_version",
            "claim-1-v1",
            "claim_version_of",
            "on_source_change",
        ));
        let claim_1_claim_version_edges = store
            .with_read_conn(|conn| {
                projection_storage::query_derivation_edges_by_source(
                    conn,
                    "claim_version",
                    "claim-1-v0",
                )
            })
            .await
            .unwrap();
        assert!(edge_exists(
            &claim_1_claim_version_edges,
            "claim_version",
            "claim-1-v1",
            "supersedes",
            "on_supersession",
        ));
        assert!(edge_exists(
            &claim_1_edges,
            "relation_version",
            "rel-1-v1",
            "supports_claim",
            "on_source_change",
        ));
        assert!(edge_exists(
            &claim_1_edges,
            "evidence_ref",
            "forge://evidence/claim-1/v1",
            "supports",
            "on_source_change",
        ));
        assert!(edge_exists(
            &claim_1_edges,
            "episode",
            "episode-1",
            "caused_by_claim",
            "on_source_change",
        ));

        let claim_2_edges = store
            .with_read_conn(|conn| {
                projection_storage::query_derivation_edges_by_source(conn, "claim", "claim-2")
            })
            .await
            .unwrap();
        assert!(edge_exists(
            &claim_2_edges,
            "evidence_ref",
            "forge://evidence/claim-2",
            "supports",
            "on_source_change",
        ));

        let relation_supersession_edges = store
            .with_read_conn(|conn| {
                projection_storage::query_derivation_edges_by_source(
                    conn,
                    "relation_version",
                    "rel-1-v0",
                )
            })
            .await
            .unwrap();
        assert!(edge_exists(
            &relation_supersession_edges,
            "relation_version",
            "rel-1-v1",
            "supersedes",
            "on_supersession",
        ));

        let entity_edges = store
            .with_read_conn(|conn| {
                projection_storage::query_derivation_edges_by_source(conn, "entity", "ent-1")
            })
            .await
            .unwrap();
        assert!(edge_exists(
            &entity_edges,
            "entity_alias",
            "ent-1",
            "canonical_alias",
            "on_alias_split",
        ));

        let split_entity_edges = store
            .with_read_conn(|conn| {
                projection_storage::query_derivation_edges_by_source(conn, "entity", "ent-split")
            })
            .await
            .unwrap();
        assert!(edge_exists(
            &split_entity_edges,
            "entity_alias",
            "ent-1",
            "alias_split_from",
            "on_alias_split",
        ));

        let superseded_by_entity_edges = store
            .with_read_conn(|conn| {
                projection_storage::query_derivation_edges_by_source(conn, "entity", "ent-old")
            })
            .await
            .unwrap();
        assert!(edge_exists(
            &superseded_by_entity_edges,
            "entity_alias",
            "ent-1",
            "alias_superseded_by",
            "on_supersession",
        ));
    }

    #[tokio::test]
    async fn bounded_invalidation_targets_expected_derived_rows() {
        let (store, _dir) = test_store();
        store
            .import_projection_batch_json_compat(&derivation_rich_batch())
            .await
            .unwrap();

        let invalidated_before = store
            .with_read_conn(|conn| projection_storage::list_invalidated_targets(conn, 100))
            .await
            .unwrap();
        assert!(invalidated_before.is_empty());

        let invalidated_count = store
            .invalidate_derivations("claim", "claim-1", "on_source_change", "test")
            .await
            .unwrap();
        assert!(invalidated_count > 0);

        let invalidated_after = store
            .with_read_conn(|conn| projection_storage::list_invalidated_targets(conn, 100))
            .await
            .unwrap();

        assert_eq!(invalidated_after.len(), invalidated_count);
        assert!(invalidated_after
            .iter()
            .all(|edge| edge.source_id == "claim-1"));
        assert!(invalidated_after
            .iter()
            .any(|edge| edge.target_kind == "claim_version" && edge.target_id == "claim-1-v1"));
        assert!(invalidated_after
            .iter()
            .any(|edge| edge.target_kind == "relation_version" && edge.target_id == "rel-1-v1"));
        assert!(invalidated_after
            .iter()
            .any(|edge| edge.target_kind == "evidence_ref"
                && edge.target_id == "forge://evidence/claim-1/v1"));
    }
}
