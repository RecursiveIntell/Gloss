//! Tests for V11 projection storage and import.
#![allow(clippy::expect_used)]
//!
//! Covers:
//! - Projection batch import (claim versions, relation versions, entity aliases, evidence refs)
//! - Idempotent re-import
//! - V11 schema migration from V10
//! - Projection import log tracking
//! - Preferred-open uniqueness
//! - Duplicate-but-not-identical envelopes
//! - Mid-import rollback (invalid record)
//! - Human-confirmed-final protection
//! - Legacy path still works (backward compatibility)

#![allow(deprecated)]

use forge_memory_bridge::PROJECTION_IMPORT_BATCH_V1_SCHEMA;
use semantic_memory::compat::compat_trace_id::TraceId;
use semantic_memory::compat::legacy_import_envelope::{ImportEnvelope, ImportRecord, ImportStatus};
use semantic_memory::{MemoryConfig, MemoryStore, MockEmbedder, ProjectionQuery};
use stack_ids::{ClaimId, ClaimVersionId, EnvelopeId, ScopeKey};
use tempfile::TempDir;
use tokio::time::{sleep, Duration};

fn test_store() -> (MemoryStore, TempDir) {
    let dir = TempDir::new().unwrap();
    let config = MemoryConfig {
        base_dir: dir.path().to_path_buf(),
        ..Default::default()
    };
    let store = MemoryStore::open_with_embedder(config, Box::new(MockEmbedder::new(768))).unwrap();
    (store, dir)
}

fn make_claim_batch(envelope_id: &str, claim_id: &str, content: &str) -> String {
    serde_json::json!({
        "source_envelope_id": envelope_id,
        "schema_version": PROJECTION_IMPORT_BATCH_V1_SCHEMA,
        "export_schema_version": "export_envelope_v1",
        "content_digest": format!("digest-{envelope_id}"),
        "source_authority": "forge",
        "scope_key": { "namespace": "test-ns" },
        "trace_ctx": { "trace_id": "trace-001" },
        "source_exported_at": "2026-03-07T00:00:00Z",
        "transformed_at": "2026-03-07T00:00:01Z",
        "records": [
            {
                "kind": "claim_version",
                "claim_id": claim_id,
                "claim_version_id": format!("{claim_id}-v1"),
                "claim_state": "active",
                "projection_family": "forge_verification",
                "subject_entity_id": "ent-1",
                "predicate": "has_type",
                "object_anchor": "function",
                "scope_key": { "namespace": "test-ns" },
                "valid_from": "2026-01-01T00:00:00Z",
                "valid_to": null,
                "preferred_open": true,
                "source_envelope_id": envelope_id,
                "source_authority": "forge",
                "freshness": "current",
                "contradiction_status": "none",
                "content": content,
                "confidence": 0.95
            }
        ]
    })
    .to_string()
}

fn make_multi_record_batch(envelope_id: &str) -> String {
    serde_json::json!({
        "source_envelope_id": envelope_id,
        "schema_version": PROJECTION_IMPORT_BATCH_V1_SCHEMA,
        "export_schema_version": "export_envelope_v1",
        "content_digest": format!("digest-multi-{envelope_id}"),
        "source_authority": "forge",
        "scope_key": { "namespace": "test-ns" },
        "source_exported_at": "2026-03-07T00:00:00Z",
        "transformed_at": "2026-03-07T00:00:01Z",
        "records": [
            {
                "kind": "claim_version",
                "claim_id": "claim-1",
                "claim_version_id": "claim-1-v1",
                "claim_state": "active",
                "projection_family": "forge",
                "subject_entity_id": "ent-1",
                "predicate": "p1",
                "object_anchor": "v1",
                "preferred_open": true,
                "freshness": "current",
                "contradiction_status": "none",
                "content": "claim one",
                "confidence": 0.9
            },
            {
                "kind": "relation_version",
                "relation_version_id": "rel-1-v1",
                "subject_entity_id": "ent-1",
                "predicate": "depends_on",
                "object_anchor": "ent-2",
                "preferred_open": true,
                "source_confidence": 0.8,
                "projection_family": "forge",
                "freshness": "current",
                "contradiction_status": "none"
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
                "is_human_confirmed_final": false
            },
            {
                "kind": "evidence_ref",
                "claim_id": "claim-1",
                "fetch_handle": "forge://evidence/run-42/artifact-7",
                "source_authority": "forge"
            },
            {
                "kind": "episode",
                "episode_id": "episode-1",
                "document_id": "doc-1",
                "cause_ids": ["claim-1"],
                "effect_type": "code_change",
                "outcome": "success",
                "confidence": 0.7,
                "experiment_id": "exp-1"
            }
        ]
    })
    .to_string()
}

fn make_multi_record_batch_collision(
    envelope_id: &str,
    content_digest: &str,
    marker: &str,
    source_exported_at: &str,
    transformed_at: &str,
) -> String {
    serde_json::json!({
        "source_envelope_id": envelope_id,
        "schema_version": PROJECTION_IMPORT_BATCH_V1_SCHEMA,
        "export_schema_version": "export_envelope_v1",
        "content_digest": content_digest,
        "source_authority": "forge",
        "scope_key": { "namespace": "test-ns" },
        "source_exported_at": source_exported_at,
        "transformed_at": transformed_at,
        "records": [
            {
                "kind": "claim_version",
                "claim_id": format!("claim-{marker}"),
                "claim_version_id": format!("claim-{marker}-v1"),
                "claim_state": "active",
                "projection_family": "forge",
                "subject_entity_id": "ent-collision",
                "predicate": "has_type",
                "object_anchor": format!("function-{marker}"),
                "preferred_open": true,
                "freshness": "current",
                "contradiction_status": "none",
                "content": format!("claim content {marker}"),
                "confidence": 0.9
            },
            {
                "kind": "relation_version",
                "relation_version_id": format!("rel-{marker}-v1"),
                "subject_entity_id": "ent-collision",
                "predicate": "depends_on",
                "object_anchor": format!("ent-collision-target-{marker}"),
                "preferred_open": true,
                "source_confidence": 0.8,
                "projection_family": "forge",
                "freshness": "current",
                "contradiction_status": "none"
            },
            {
                "kind": "entity_alias",
                "canonical_entity_id": format!("ent-{marker}"),
                "alias_text": format!("Entity {marker}"),
                "alias_source": "forge_extraction",
                "confidence": 0.9,
                "merge_decision": { "automated": { "algorithm": "bridge_default" } },
                "scope": { "namespace": "test-ns" },
                "review_state": "unreviewed",
                "is_human_confirmed": false,
                "is_human_confirmed_final": false
            },
            {
                "kind": "evidence_ref",
                "claim_id": format!("claim-{marker}"),
                "fetch_handle": format!("forge://evidence/{marker}"),
                "source_authority": "forge"
            },
            {
                "kind": "episode",
                "episode_id": format!("episode-{marker}"),
                "document_id": format!("doc-{marker}"),
                "cause_ids": [format!("claim-{marker}")],
                "effect_type": "code_change",
                "outcome": "success",
                "confidence": 0.7,
                "experiment_id": null
            }
        ]
    })
    .to_string()
}

#[allow(clippy::too_many_arguments)]
fn make_scoped_claim_batch(
    envelope_id: &str,
    scope_key: &ScopeKey,
    claim_id: &str,
    claim_version_id: &str,
    content: &str,
    valid_from: &str,
    valid_to: Option<&str>,
    preferred_open: bool,
) -> String {
    serde_json::json!({
        "source_envelope_id": envelope_id,
        "schema_version": PROJECTION_IMPORT_BATCH_V1_SCHEMA,
        "export_schema_version": "export_envelope_v1",
        "content_digest": format!("digest-{envelope_id}"),
        "source_authority": "forge",
        "scope_key": scope_key,
        "source_exported_at": "2026-03-07T00:00:00Z",
        "transformed_at": "2026-03-07T00:00:01Z",
        "records": [{
            "kind": "claim_version",
            "claim_id": claim_id,
            "claim_version_id": claim_version_id,
            "claim_state": "active",
            "projection_family": "forge_verification",
            "subject_entity_id": "ent-scope",
            "predicate": "has_type",
            "object_anchor": "function",
            "scope_key": scope_key,
            "valid_from": valid_from,
            "valid_to": valid_to,
            "preferred_open": preferred_open,
            "source_envelope_id": envelope_id,
            "source_authority": "forge",
            "freshness": "current",
            "contradiction_status": "none",
            "content": content,
            "confidence": 0.95
        }]
    })
    .to_string()
}

fn make_verification_relation_batch(
    envelope_id: &str,
    source_exported_at: &str,
    transformed_at: &str,
) -> String {
    serde_json::json!({
        "source_envelope_id": envelope_id,
        "schema_version": PROJECTION_IMPORT_BATCH_V1_SCHEMA,
        "export_schema_version": "export_envelope_v1",
        "content_digest": format!("digest-{envelope_id}"),
        "source_authority": "forge",
        "scope_key": { "namespace": "test-ns" },
        "source_exported_at": source_exported_at,
        "transformed_at": transformed_at,
        "records": [
            {
                "kind": "relation_version",
                "relation_version_id": "rel-baseline-v1",
                "subject_entity_id": "ent-verification",
                "predicate": "verification_trial_baseline",
                "object_anchor": {
                    "trial_id": "trial-baseline-1",
                    "attempt_id": "attempt-verification-1",
                    "baseline_or_patch": "Baseline",
                    "completed": true
                },
                "scope_key": { "namespace": "test-ns" },
                "preferred_open": true,
                "contradiction_status": "none",
                "source_confidence": 0.9,
                "projection_family": "forge_verification",
                "source_envelope_id": envelope_id,
                "source_authority": "forge",
                "freshness": "current",
                "metadata": {
                    "bundle_id": "bundle-verification-1",
                    "attempt_id": "attempt-verification-1",
                    "trial_id": "trial-baseline-1",
                    "baseline_or_patch": "Baseline"
                }
            },
            {
                "kind": "relation_version",
                "relation_version_id": "rel-patched-v1",
                "subject_entity_id": "ent-verification",
                "predicate": "verification_trial_patched",
                "object_anchor": {
                    "trial_id": "trial-patched-1",
                    "attempt_id": "attempt-verification-1",
                    "baseline_or_patch": "Patched",
                    "completed": true
                },
                "scope_key": { "namespace": "test-ns" },
                "preferred_open": true,
                "contradiction_status": "none",
                "source_confidence": 0.91,
                "projection_family": "forge_verification",
                "source_envelope_id": envelope_id,
                "source_authority": "forge",
                "freshness": "current",
                "metadata": {
                    "bundle_id": "bundle-verification-1",
                    "attempt_id": "attempt-verification-1",
                    "trial_id": "trial-patched-1",
                    "baseline_or_patch": "Patched"
                }
            },
            {
                "kind": "relation_version",
                "relation_version_id": "rel-placebo-v1",
                "subject_entity_id": "ent-verification",
                "predicate": "verification_refutation_placebo",
                "object_anchor": {
                    "artifact_id": "ref-placebo-1",
                    "artifact_type": "Placebo",
                    "outcome": "passed",
                    "details": "no effect for placebo"
                },
                "scope_key": { "namespace": "test-ns" },
                "preferred_open": true,
                "contradiction_status": "none",
                "source_confidence": 0.92,
                "projection_family": "forge_verification",
                "source_envelope_id": envelope_id,
                "source_authority": "forge",
                "freshness": "current",
                "metadata": {
                    "bundle_id": "bundle-verification-1",
                    "artifact_id": "ref-placebo-1",
                    "outcome": "passed"
                }
            },
            {
                "kind": "relation_version",
                "relation_version_id": "rel-dummy-v1",
                "subject_entity_id": "ent-verification",
                "predicate": "verification_refutation_dummy_outcome",
                "object_anchor": {
                    "artifact_id": "ref-dummy-1",
                    "artifact_type": "DummyOutcome",
                    "outcome": "inconclusive",
                    "details": "outcome nullification check incomplete"
                },
                "scope_key": { "namespace": "test-ns" },
                "preferred_open": true,
                "contradiction_status": "none",
                "source_confidence": 0.93,
                "projection_family": "forge_verification",
                "source_envelope_id": envelope_id,
                "source_authority": "forge",
                "freshness": "current",
                "metadata": {
                    "bundle_id": "bundle-verification-1",
                    "artifact_id": "ref-dummy-1",
                    "outcome": "inconclusive"
                }
            },
            {
                "kind": "relation_version",
                "relation_version_id": "rel-subsample-v1",
                "subject_entity_id": "ent-verification",
                "predicate": "verification_refutation_subsample_stability",
                "object_anchor": {
                    "artifact_id": "ref-subsample-1",
                    "artifact_type": "SubsampleStability",
                    "outcome": "failed",
                    "details": "instability across folds"
                },
                "scope_key": { "namespace": "test-ns" },
                "preferred_open": true,
                "contradiction_status": "none",
                "source_confidence": 0.93,
                "projection_family": "forge_verification",
                "source_envelope_id": envelope_id,
                "source_authority": "forge",
                "freshness": "current",
                "metadata": {
                    "bundle_id": "bundle-verification-1",
                    "artifact_id": "ref-subsample-1",
                    "outcome": "failed"
                }
            }
        ]
    })
    .to_string()
}

#[tokio::test]
async fn import_claim_version_succeeds() {
    let (store, _dir) = test_store();
    let batch = make_claim_batch("env-001", "claim-1", "Test claim content");
    let result = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();

    assert_eq!(result.status, "complete");
    assert_eq!(result.record_count, 1);
    assert!(!result.was_duplicate);
    assert_eq!(result.source_envelope_id, "env-001");
}

#[tokio::test]
async fn import_is_idempotent() {
    let (store, _dir) = test_store();
    let batch = make_claim_batch("env-002", "claim-2", "Idempotent claim");

    let r1 = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();
    assert_eq!(r1.status, "complete");
    assert!(!r1.was_duplicate);

    let r2 = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();
    assert_eq!(r2.status, "already_imported");
    assert!(r2.was_duplicate);
}

#[tokio::test]
async fn import_multi_record_batch() {
    let (store, _dir) = test_store();
    let batch = make_multi_record_batch("env-multi");
    let result = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();

    assert_eq!(result.status, "complete");
    assert_eq!(result.record_count, 5);
    assert!(!result.was_duplicate);
}

#[tokio::test]
async fn public_projection_queries_read_imported_rows() {
    let (store, _dir) = test_store();
    let batch = make_multi_record_batch("env-queryable");
    store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();

    let query = ProjectionQuery::new(ScopeKey::namespace_only("test-ns"));
    let claims = store.query_claim_versions(query.clone()).await.unwrap();
    let relations = store.query_relation_versions(query.clone()).await.unwrap();
    let episodes = store.query_episodes(query.clone()).await.unwrap();
    let aliases = store.query_entity_aliases(query.clone()).await.unwrap();
    let evidence = store.query_evidence_refs(query).await.unwrap();

    assert_eq!(claims.len(), 1);
    assert_eq!(relations.len(), 1);
    assert_eq!(episodes.len(), 1);
    assert_eq!(aliases.len(), 1);
    assert_eq!(evidence.len(), 1);
    assert_eq!(claims[0].content, "claim one");
    assert_eq!(relations[0].predicate, "depends_on");
    assert!(
        relations[0].claim_id.is_none(),
        "missing relation claim_id must remain absent, not become an empty identifier"
    );
    assert!(
        relations[0].source_episode_id.is_none(),
        "missing relation source_episode_id must remain absent, not become an empty identifier"
    );
    assert_eq!(episodes[0].effect_type, "code_change");
    assert_eq!(aliases[0].alias_text, "Entity One");
    assert_eq!(
        evidence[0].fetch_handle,
        "forge://evidence/run-42/artifact-7"
    );
    assert!(
        evidence[0].claim_version_id.is_none(),
        "missing evidence claim_version_id must remain absent, not become an empty identifier"
    );
    assert_eq!(
        claims[0].source_exported_at.as_deref(),
        Some("2026-03-07T00:00:00Z")
    );
    assert_eq!(
        claims[0].transformed_at.as_deref(),
        Some("2026-03-07T00:00:01Z")
    );
}

#[tokio::test]
async fn projection_queries_enforce_full_scope() {
    let (store, _dir) = test_store();
    let matching_scope = ScopeKey {
        namespace: "test-ns".into(),
        domain: Some("code".into()),
        workspace_id: Some("ws-1".into()),
        repo_id: Some("repo-1".into()),
    };
    let other_scope = ScopeKey {
        namespace: "test-ns".into(),
        domain: Some("docs".into()),
        workspace_id: Some("ws-2".into()),
        repo_id: Some("repo-2".into()),
    };

    for batch in [
        make_scoped_claim_batch(
            "env-scope-hit",
            &matching_scope,
            "claim-scope-hit",
            "claim-scope-hit-v1",
            "scoped projection hit",
            "2026-01-01T00:00:00Z",
            None,
            true,
        ),
        make_scoped_claim_batch(
            "env-scope-miss",
            &other_scope,
            "claim-scope-miss",
            "claim-scope-miss-v1",
            "scoped projection miss",
            "2026-01-01T00:00:00Z",
            None,
            true,
        ),
    ] {
        store
            .import_projection_batch_json_compat(&batch)
            .await
            .unwrap();
    }

    let mut query = ProjectionQuery::new(matching_scope.clone());
    query.text_query = Some("scoped projection".into());
    let claims = store.query_claim_versions(query).await.unwrap();

    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].content, "scoped projection hit");
    assert_eq!(claims[0].scope_key, matching_scope);
}

#[tokio::test]
async fn claim_query_filters_by_recorded_at_cutoff() {
    let (store, _dir) = test_store();
    let scope_key = ScopeKey::namespace_only("test-ns");

    let batch_old = make_scoped_claim_batch(
        "env-bitemporal-old",
        &scope_key,
        "claim-bitemporal-old",
        "claim-bitemporal-old-v1",
        "recorded-at cutoff claim historical",
        "2026-01-01T00:00:00Z",
        None,
        true,
    );
    let batch_new = make_scoped_claim_batch(
        "env-bitemporal-new",
        &scope_key,
        "claim-bitemporal-new",
        "claim-bitemporal-new-v1",
        "recorded-at cutoff claim updated",
        "2026-02-01T00:00:00Z",
        None,
        true,
    );

    store
        .import_projection_batch_json_compat(&batch_old)
        .await
        .unwrap();
    sleep(Duration::from_secs(1)).await;
    store
        .import_projection_batch_json_compat(&batch_new)
        .await
        .unwrap();

    let import_log = store
        .query_projection_imports(Some("test-ns"), 10)
        .await
        .unwrap();
    assert!(import_log.len() >= 2, "expected two projection imports");
    let oldest_imported_at = import_log
        .iter()
        .map(|entry| entry.imported_at.clone())
        .min()
        .expect("at least one projection import");
    let latest_imported_at = import_log
        .iter()
        .map(|entry| entry.imported_at.clone())
        .max()
        .expect("at least one projection import");

    let mut historical = ProjectionQuery::new(scope_key.clone());
    historical.text_query = Some("recorded-at cutoff claim".into());
    historical.valid_at = Some("2026-03-01T00:00:00Z".into());
    historical.recorded_at_or_before = Some(oldest_imported_at);
    let historical_claims = store.query_claim_versions(historical).await.unwrap();

    assert_eq!(
        historical_claims.len(),
        1,
        "earliest recorded-at cutoff should exclude claims imported later"
    );
    assert_eq!(
        historical_claims[0].content,
        "recorded-at cutoff claim historical"
    );

    let mut current = ProjectionQuery::new(scope_key);
    current.text_query = Some("recorded-at cutoff claim".into());
    current.valid_at = Some("2026-03-01T00:00:00Z".into());
    current.recorded_at_or_before = Some(latest_imported_at);
    let current_claims = store.query_claim_versions(current).await.unwrap();

    assert_eq!(
        current_claims.len(),
        2,
        "latest recorded-at cutoff should include both rows"
    );
    assert!(
        current_claims
            .iter()
            .any(|claim| claim.content == "recorded-at cutoff claim historical"),
        "historical row should remain visible at a later cutoff"
    );
    assert!(
        current_claims
            .iter()
            .any(|claim| claim.content == "recorded-at cutoff claim updated"),
        "latest row should be visible at the later cutoff"
    );
}

#[tokio::test]
async fn claim_query_valid_at_filters_versions() {
    let (store, _dir) = test_store();
    let scope_key = ScopeKey::namespace_only("test-ns");

    for batch in [
        make_scoped_claim_batch(
            "env-claim-old",
            &scope_key,
            "claim-versioned",
            "claim-versioned-v1",
            "versioned claim old state",
            "2026-01-01T00:00:00Z",
            Some("2026-02-01T00:00:00Z"),
            false,
        ),
        make_scoped_claim_batch(
            "env-claim-current",
            &scope_key,
            "claim-versioned",
            "claim-versioned-v2",
            "versioned claim current state",
            "2026-02-01T00:00:00Z",
            None,
            true,
        ),
    ] {
        store
            .import_projection_batch_json_compat(&batch)
            .await
            .unwrap();
    }

    let mut historical = ProjectionQuery::new(scope_key.clone());
    historical.text_query = Some("versioned claim".into());
    historical.valid_at = Some("2026-01-15T00:00:00Z".into());
    let historical_claims = store.query_claim_versions(historical).await.unwrap();

    let mut current = ProjectionQuery::new(scope_key);
    current.text_query = Some("versioned claim".into());
    current.valid_at = Some("2026-03-15T00:00:00Z".into());
    let current_claims = store.query_claim_versions(current).await.unwrap();

    assert_eq!(historical_claims.len(), 1);
    assert_eq!(historical_claims[0].content, "versioned claim old state");
    assert_eq!(current_claims.len(), 1);
    assert_eq!(current_claims[0].content, "versioned claim current state");
}

#[tokio::test]
async fn claim_query_filters_by_claim_version_id() {
    let (store, _dir) = test_store();
    let scope_key = ScopeKey::namespace_only("test-ns");

    for batch in [
        make_scoped_claim_batch(
            "env-claim-version-filter-old",
            &scope_key,
            "claim-version-filter",
            "claim-version-filter-v1",
            "claim version filter old",
            "2026-01-01T00:00:00Z",
            Some("2026-02-01T00:00:00Z"),
            false,
        ),
        make_scoped_claim_batch(
            "env-claim-version-filter-current",
            &scope_key,
            "claim-version-filter",
            "claim-version-filter-v2",
            "claim version filter current",
            "2026-02-01T00:00:00Z",
            None,
            true,
        ),
    ] {
        store
            .import_projection_batch_json_compat(&batch)
            .await
            .unwrap();
    }

    let mut query = ProjectionQuery::new(scope_key);
    query.claim_id = Some(ClaimId::new("claim-version-filter"));
    query.claim_version_id = Some(ClaimVersionId::new("claim-version-filter-v1"));
    let claims = store.query_claim_versions(query).await.unwrap();

    assert_eq!(
        claims.len(),
        1,
        "claim_version_id filter should narrow to the requested imported version"
    );
    assert_eq!(
        claims[0].claim_version_id.as_str(),
        "claim-version-filter-v1"
    );
    assert_eq!(claims[0].content, "claim version filter old");
}

#[tokio::test]
async fn duplicate_but_different_digest_both_import() {
    let (store, _dir) = test_store();

    let batch1 = make_claim_batch("env-dup", "claim-a", "Content A");
    let r1 = store
        .import_projection_batch_json_compat(&batch1)
        .await
        .unwrap();
    assert_eq!(r1.status, "complete");

    // Same envelope_id but different content_digest
    let batch2 = serde_json::json!({
        "source_envelope_id": "env-dup",
        "schema_version": PROJECTION_IMPORT_BATCH_V1_SCHEMA,
        "export_schema_version": "export_envelope_v1",
        "content_digest": "different-digest",
        "source_authority": "forge",
        "scope_key": { "namespace": "test-ns" },
        "source_exported_at": "2026-03-07T00:00:00Z",
        "transformed_at": "2026-03-07T00:00:01Z",
        "records": [{
            "kind": "claim_version",
            "claim_id": "claim-b",
            "claim_version_id": "claim-b-v1",
            "claim_state": "active",
            "projection_family": "forge",
            "subject_entity_id": "ent-2",
            "predicate": "p2",
            "object_anchor": "v2",
            "preferred_open": true,
            "freshness": "current",
            "contradiction_status": "none",
            "content": "Content B",
            "confidence": 0.8
        }]
    })
    .to_string();

    let r2 = store
        .import_projection_batch_json_compat(&batch2)
        .await
        .unwrap();
    assert_eq!(r2.status, "complete");
    assert!(!r2.was_duplicate);
}

#[tokio::test]
async fn duplicate_envelope_id_different_digests_do_not_duplicate_queries() {
    let (store, _dir) = test_store();

    let batch_a = make_multi_record_batch_collision(
        "env-dup-overlap",
        "digest-dup-a",
        "A",
        "2026-03-07T00:00:00Z",
        "2026-03-07T00:00:01Z",
    );
    let batch_b = make_multi_record_batch_collision(
        "env-dup-overlap",
        "digest-dup-b",
        "B",
        "2026-03-08T00:00:00Z",
        "2026-03-08T00:00:01Z",
    );

    store
        .import_projection_batch_json_compat(&batch_a)
        .await
        .unwrap();
    sleep(Duration::from_secs(1)).await;
    store
        .import_projection_batch_json_compat(&batch_b)
        .await
        .unwrap();

    let query = ProjectionQuery::new(ScopeKey::namespace_only("test-ns"));
    let claims = store.query_claim_versions(query.clone()).await.unwrap();
    let relations = store.query_relation_versions(query.clone()).await.unwrap();
    let episodes = store.query_episodes(query.clone()).await.unwrap();
    let aliases = store.query_entity_aliases(query.clone()).await.unwrap();
    let evidence = store.query_evidence_refs(query).await.unwrap();

    assert_eq!(claims.len(), 2);
    assert_eq!(relations.len(), 2);
    assert_eq!(episodes.len(), 2);
    assert_eq!(aliases.len(), 2);
    assert_eq!(evidence.len(), 2);

    let claim_a = claims
        .iter()
        .find(|row| row.claim_id.as_str() == "claim-A")
        .expect("claim-A should be present");
    let claim_b = claims
        .iter()
        .find(|row| row.claim_id.as_str() == "claim-B")
        .expect("claim-B should be present");
    assert_eq!(
        claim_a.source_exported_at.as_deref(),
        Some("2026-03-07T00:00:00Z")
    );
    assert_eq!(
        claim_b.source_exported_at.as_deref(),
        Some("2026-03-08T00:00:00Z")
    );

    let alias_a = aliases
        .iter()
        .find(|row| row.alias_text == "Entity A")
        .expect("alias A should be present");
    let alias_b = aliases
        .iter()
        .find(|row| row.alias_text == "Entity B")
        .expect("alias B should be present");
    assert_eq!(
        alias_a.source_exported_at.as_deref(),
        Some("2026-03-07T00:00:00Z")
    );
    assert_eq!(
        alias_b.source_exported_at.as_deref(),
        Some("2026-03-08T00:00:00Z")
    );

    let evidence_a = evidence
        .iter()
        .find(|row| row.fetch_handle == "forge://evidence/A")
        .expect("evidence A should be present");
    let evidence_b = evidence
        .iter()
        .find(|row| row.fetch_handle == "forge://evidence/B")
        .expect("evidence B should be present");
    assert_eq!(
        evidence_a.source_exported_at.as_deref(),
        Some("2026-03-07T00:00:00Z")
    );
    assert_eq!(
        evidence_b.source_exported_at.as_deref(),
        Some("2026-03-08T00:00:00Z")
    );
}

#[tokio::test]
async fn invalid_batch_json_rejected() {
    let (store, _dir) = test_store();
    let err = store
        .import_projection_batch_json_compat("not valid json")
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
}

#[tokio::test]
async fn missing_required_fields_rejected() {
    let (store, _dir) = test_store();
    let batch = serde_json::json!({
        "source_envelope_id": "env-bad",
        "records": []
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
}

#[tokio::test]
async fn unknown_record_kind_rejected() {
    let (store, _dir) = test_store();
    let batch = serde_json::json!({
        "source_envelope_id": "env-unknown",
        "schema_version": PROJECTION_IMPORT_BATCH_V1_SCHEMA,
        "export_schema_version": "export_envelope_v1",
        "content_digest": "digest-unknown",
        "source_authority": "forge",
        "scope_key": { "namespace": "test-ns" },
        "source_exported_at": "2026-03-07T00:00:00Z",
        "transformed_at": "2026-03-07T00:00:01Z",
        "records": [{
            "kind": "unknown_type",
            "data": "foo"
        }]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
}

#[tokio::test]
async fn legacy_import_envelope_still_works() {
    // Backward compatibility: the old import_envelope() path must still function
    let (store, _dir) = test_store();

    let envelope = ImportEnvelope {
        envelope_id: EnvelopeId::new("legacy-env-001"),
        schema_version: "1.0".into(),
        content_digest: "legacy-digest-001".into(),
        source_authority: "forge".into(),
        trace_id: Some(TraceId::new("trace-legacy")),
        namespace: "test-ns".into(),
        records: vec![ImportRecord::Fact {
            content: "Legacy fact content".into(),
            source: Some("test".into()),
            metadata: None,
        }],
    };

    let receipt = store.import_envelope(&envelope).await.unwrap();
    assert_eq!(receipt.status, ImportStatus::Complete);
    assert_eq!(receipt.record_count, 1);
    assert!(!receipt.was_duplicate);
}

#[tokio::test]
async fn entity_alias_review_state_persisted() {
    let (store, _dir) = test_store();

    let batch = serde_json::json!({
        "source_envelope_id": "env-alias",
        "schema_version": PROJECTION_IMPORT_BATCH_V1_SCHEMA,
        "export_schema_version": "export_envelope_v1",
        "content_digest": "digest-alias",
        "source_authority": "forge",
        "scope_key": { "namespace": "test-ns" },
        "source_exported_at": "2026-03-07T00:00:00Z",
        "transformed_at": "2026-03-07T00:00:01Z",
        "records": [{
            "kind": "entity_alias",
            "canonical_entity_id": "ent-1",
            "alias_text": "Entity One",
            "alias_source": "forge_extraction",
            "confidence": 0.85,
            "merge_decision": { "automated": { "algorithm": "exact_match" } },
            "scope": { "namespace": "test-ns" },
            "review_state": "pending_review",
            "is_human_confirmed": false,
            "is_human_confirmed_final": false
        }]
    })
    .to_string();

    let result = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();
    assert_eq!(result.status, "complete");
    // The alias is imported with pending_review state (durable, survives restart)
    // Re-opening the store should still find it
}

#[tokio::test]
async fn evidence_ref_audit_only() {
    let (store, _dir) = test_store();

    let batch = serde_json::json!({
        "source_envelope_id": "env-evidence",
        "schema_version": PROJECTION_IMPORT_BATCH_V1_SCHEMA,
        "export_schema_version": "export_envelope_v1",
        "content_digest": "digest-evidence",
        "source_authority": "forge",
        "scope_key": { "namespace": "test-ns" },
        "source_exported_at": "2026-03-07T00:00:00Z",
        "transformed_at": "2026-03-07T00:00:01Z",
        "records": [{
            "kind": "evidence_ref",
            "claim_id": "claim-x",
            "claim_version_id": "claim-x-v1",
            "fetch_handle": "forge://evidence/run-42/artifact-7",
            "source_authority": "forge"
        }]
    })
    .to_string();

    let result = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();
    assert_eq!(result.status, "complete");
    assert_eq!(result.record_count, 1);
}

#[tokio::test]
async fn relation_versions_query_verification_trials_and_refutations() {
    let (store, _dir) = test_store();
    let batch = make_verification_relation_batch(
        "env-verification-relations",
        "2026-03-07T00:00:00Z",
        "2026-03-07T00:00:01Z",
    );

    store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();

    let mut query = ProjectionQuery::new(ScopeKey::namespace_only("test-ns"));
    query.text_query = Some("verification".into());
    let relations = store.query_relation_versions(query).await.unwrap();

    assert_eq!(relations.len(), 5);
    assert!(
        relations
            .iter()
            .any(|relation| relation.predicate == "verification_trial_baseline"),
        "baseline trial relation should be present"
    );
    assert!(
        relations
            .iter()
            .any(|relation| relation.predicate == "verification_trial_patched"),
        "patched trial relation should be present"
    );
    assert!(
        relations
            .iter()
            .any(|relation| relation.predicate == "verification_refutation_placebo"),
        "placebo refutation relation should be present"
    );
    assert!(
        relations
            .iter()
            .any(|relation| relation.predicate == "verification_refutation_dummy_outcome"),
        "dummy outcome refutation relation should be present"
    );
    assert!(
        relations
            .iter()
            .any(|relation| relation.predicate == "verification_refutation_subsample_stability"),
        "subsample stability refutation relation should be present"
    );

    let baseline = relations
        .iter()
        .find(|relation| relation.predicate == "verification_trial_baseline")
        .expect("baseline trial relation should exist");
    let patched = relations
        .iter()
        .find(|relation| relation.predicate == "verification_trial_patched")
        .expect("patched trial relation should exist");
    let dummy = relations
        .iter()
        .find(|relation| relation.predicate == "verification_refutation_dummy_outcome")
        .expect("dummy outcome refutation relation should exist");
    let subsample = relations
        .iter()
        .find(|relation| relation.predicate == "verification_refutation_subsample_stability")
        .expect("subsample refutation relation should exist");

    assert_eq!(
        baseline
            .metadata
            .as_ref()
            .and_then(|m| m.get("attempt_id"))
            .and_then(|value| value.as_str()),
        Some("attempt-verification-1")
    );
    assert_eq!(
        baseline
            .metadata
            .as_ref()
            .and_then(|m| m.get("trial_id"))
            .and_then(|value| value.as_str()),
        Some("trial-baseline-1")
    );
    assert_eq!(
        patched
            .metadata
            .as_ref()
            .and_then(|m| m.get("trial_id"))
            .and_then(|value| value.as_str()),
        Some("trial-patched-1")
    );
    assert_eq!(
        dummy
            .metadata
            .as_ref()
            .and_then(|m| m.get("outcome"))
            .and_then(|value| value.as_str()),
        Some("inconclusive")
    );
    assert_eq!(
        subsample
            .metadata
            .as_ref()
            .and_then(|m| m.get("outcome"))
            .and_then(|value| value.as_str()),
        Some("failed")
    );
}
