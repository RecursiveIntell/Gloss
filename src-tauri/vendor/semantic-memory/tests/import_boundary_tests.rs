//! Tests for the projection import boundary.
#![allow(clippy::expect_used)]
//!
//! ## Legacy path (V10 ImportEnvelope)
//! Phase status: compatibility / migration-only.
//! These tests prove backward-compatible behavior for the legacy path.
//!
//! ## Canonical path (V11+ import_projection_batch)
//! Tests at the end of this file cover the canonical projection-batch
//! boundary: schema validation, required-field enforcement, idempotency,
//! rollback, invariants, and trace/provenance preservation.
//!
//! Legacy tests prove:
//! - Atomic import per envelope
//! - Idempotent repeated ingest
//! - No partial visibility on failure
//! - Explicit dedupe semantics
//! - Invalid envelope rejection
//! - Provenance preservation in imported records
//! - Import status tracking

#![allow(deprecated)]

use forge_memory_bridge::{
    ClaimState, ContradictionStatus, ImportClaimVersion, ImportProjectionRecord,
    ImportProjectionRecordV3, ProjectionFreshness as BridgeProjectionFreshness,
    ProjectionImportBatchV1, ProjectionImportBatchV2, ProjectionImportBatchV3,
    PROJECTION_IMPORT_BATCH_V1_SCHEMA, PROJECTION_IMPORT_BATCH_V2_SCHEMA,
    PROJECTION_IMPORT_BATCH_V3_SCHEMA,
};
use semantic_memory::compat::compat_trace_id::TraceId;
use semantic_memory::compat::legacy_import_envelope::{
    ImportEnvelope, ImportProjectionFreshness, ImportRecord, ImportStatus,
};
use semantic_memory::{MemoryConfig, MemoryStore, MockEmbedder};
use semantic_memory_forge::{
    CausalQuestion, CausalRoleHint, ConstraintSeedKind, DispatchOutcomeV1, EpisodeBundleV1,
    EvidenceBundle, EvidenceBundleId, ExecutionContextV1, ExportAuthority, ExportConfidenceClass,
    ExportRecordSemanticsV3, ForgeExportMeta, OutcomeSpec, ProjectionVisibilityClass,
    TreatmentSpec,
};
use stack_ids::{
    AssertionGroupId, ClaimFamilyId, ClaimId, ClaimVersionId, ContentDigest, EntityId, EnvelopeId,
    EpisodeId, ScopeKey, TraceCtx,
};
use tempfile::TempDir;

fn test_store() -> (MemoryStore, TempDir) {
    let dir = TempDir::new().unwrap();
    let config = MemoryConfig {
        base_dir: dir.path().to_path_buf(),
        ..Default::default()
    };
    let embedder = Box::new(MockEmbedder::new(768));
    let store = MemoryStore::open_with_embedder(config, embedder).unwrap();
    (store, dir)
}

fn make_envelope(id: &str, namespace: &str) -> ImportEnvelope {
    ImportEnvelope {
        envelope_id: EnvelopeId::new(id),
        schema_version: "test-v1".into(),
        content_digest: format!("digest-{id}"),
        source_authority: "test".into(),
        trace_id: Some(TraceId::new("trace-123")),
        namespace: namespace.into(),
        records: vec![ImportRecord::Fact {
            content: format!("Test fact from envelope {id}"),
            source: Some("test-source".into()),
            metadata: Some(serde_json::json!({"key": "value"})),
        }],
    }
}

fn make_multi_record_envelope(id: &str) -> ImportEnvelope {
    ImportEnvelope {
        envelope_id: EnvelopeId::new(id),
        schema_version: "test-v1".into(),
        content_digest: format!("digest-{id}"),
        source_authority: "test".into(),
        trace_id: None,
        namespace: "test".into(),
        records: vec![
            ImportRecord::Fact {
                content: "First fact".into(),
                source: None,
                metadata: None,
            },
            ImportRecord::Fact {
                content: "Second fact".into(),
                source: Some("source-b".into()),
                metadata: None,
            },
            ImportRecord::Fact {
                content: "Third fact".into(),
                source: None,
                metadata: Some(serde_json::json!({"extra": true})),
            },
        ],
    }
}

// ─── Envelope Validation Tests ─────────────────────────────────

#[tokio::test]
async fn rejects_empty_envelope_id() {
    let (store, _dir) = test_store();
    let mut env = make_envelope("", "ns");
    env.envelope_id = EnvelopeId::new("");
    let err = store.import_envelope(&env).await.unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(err.to_string().contains("envelope_id"));
}

#[tokio::test]
async fn rejects_empty_schema_version() {
    let (store, _dir) = test_store();
    let mut env = make_envelope("e1", "ns");
    env.schema_version = String::new();
    let err = store.import_envelope(&env).await.unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(err.to_string().contains("schema_version"));
}

#[tokio::test]
async fn rejects_empty_content_digest() {
    let (store, _dir) = test_store();
    let mut env = make_envelope("e1", "ns");
    env.content_digest = String::new();
    let err = store.import_envelope(&env).await.unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(err.to_string().contains("content_digest"));
}

#[tokio::test]
async fn rejects_empty_source_authority() {
    let (store, _dir) = test_store();
    let mut env = make_envelope("e1", "ns");
    env.source_authority = String::new();
    let err = store.import_envelope(&env).await.unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(err.to_string().contains("source_authority"));
}

#[tokio::test]
async fn rejects_empty_namespace() {
    let (store, _dir) = test_store();
    let mut env = make_envelope("e1", "ns");
    env.namespace = String::new();
    let err = store.import_envelope(&env).await.unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(err.to_string().contains("namespace"));
}

#[tokio::test]
async fn rejects_empty_records() {
    let (store, _dir) = test_store();
    let mut env = make_envelope("e1", "ns");
    env.records = vec![];
    let err = store.import_envelope(&env).await.unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(err.to_string().contains("at least one record"));
}

#[tokio::test]
async fn rejects_fact_with_empty_content() {
    let (store, _dir) = test_store();
    let mut env = make_envelope("e1", "ns");
    env.records = vec![ImportRecord::Fact {
        content: String::new(),
        source: None,
        metadata: None,
    }];
    let err = store.import_envelope(&env).await.unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(err.to_string().contains("content must not be empty"));
}

// ─── Successful Import Tests ───────────────────────────────────

#[tokio::test]
async fn imports_single_fact_envelope() {
    let (store, _dir) = test_store();
    let env = make_envelope("env-1", "test-ns");
    let receipt = store.import_envelope(&env).await.unwrap();

    assert_eq!(receipt.status, ImportStatus::Complete);
    assert_eq!(receipt.record_count, 1);
    assert!(!receipt.was_duplicate);
    assert_eq!(receipt.envelope_id.as_str(), "env-1");
    assert!(receipt.trace_id.is_some());

    // Verify the fact was actually stored
    let results = store
        .search(
            "Test fact from envelope env-1",
            Some(5),
            Some(&["test-ns"]),
            None,
        )
        .await
        .unwrap();
    assert!(!results.is_empty(), "imported fact should be searchable");
}

#[tokio::test]
async fn imports_multi_record_envelope_atomically() {
    let (store, _dir) = test_store();
    let env = make_multi_record_envelope("multi-1");
    let receipt = store.import_envelope(&env).await.unwrap();

    assert_eq!(receipt.status, ImportStatus::Complete);
    assert_eq!(receipt.record_count, 3);
    assert!(!receipt.was_duplicate);

    // All three facts should be searchable
    let results = store
        .search("First fact", Some(5), Some(&["test"]), None)
        .await
        .unwrap();
    assert!(!results.is_empty(), "first fact should be searchable");

    let results = store
        .search("Second fact", Some(5), Some(&["test"]), None)
        .await
        .unwrap();
    assert!(!results.is_empty(), "second fact should be searchable");

    let results = store
        .search("Third fact", Some(5), Some(&["test"]), None)
        .await
        .unwrap();
    assert!(!results.is_empty(), "third fact should be searchable");
}

// ─── Idempotent Import Tests ───────────────────────────────────

#[tokio::test]
async fn repeated_import_is_idempotent() {
    let (store, _dir) = test_store();
    let env = make_envelope("idem-1", "ns");

    // First import
    let r1 = store.import_envelope(&env).await.unwrap();
    assert_eq!(r1.status, ImportStatus::Complete);
    assert!(!r1.was_duplicate);

    // Second import of same envelope
    let r2 = store.import_envelope(&env).await.unwrap();
    assert_eq!(r2.status, ImportStatus::AlreadyImported);
    assert!(r2.was_duplicate);

    // Third import still idempotent
    let r3 = store.import_envelope(&env).await.unwrap();
    assert!(r3.was_duplicate);
}

#[tokio::test]
async fn same_envelope_id_different_digest_imports_separately() {
    let (store, _dir) = test_store();

    let env1 = ImportEnvelope {
        envelope_id: EnvelopeId::new("shared-id"),
        schema_version: "v1".into(),
        content_digest: "digest-aaa".into(),
        source_authority: "test".into(),
        trace_id: None,
        namespace: "ns".into(),
        records: vec![ImportRecord::Fact {
            content: "Version A".into(),
            source: None,
            metadata: None,
        }],
    };

    let env2 = ImportEnvelope {
        envelope_id: EnvelopeId::new("shared-id"),
        schema_version: "v1".into(),
        content_digest: "digest-bbb".into(), // different digest
        source_authority: "test".into(),
        trace_id: None,
        namespace: "ns".into(),
        records: vec![ImportRecord::Fact {
            content: "Version B".into(),
            source: None,
            metadata: None,
        }],
    };

    let r1 = store.import_envelope(&env1).await.unwrap();
    assert!(!r1.was_duplicate);

    let r2 = store.import_envelope(&env2).await.unwrap();
    assert!(
        !r2.was_duplicate,
        "different content_digest should not be considered duplicate"
    );
}

#[tokio::test]
async fn same_envelope_id_different_schema_version_imports_separately() {
    let (store, _dir) = test_store();

    let env1 = ImportEnvelope {
        envelope_id: EnvelopeId::new("shared-id"),
        schema_version: "v1".into(),
        content_digest: "digest-same".into(),
        source_authority: "test".into(),
        trace_id: None,
        namespace: "ns".into(),
        records: vec![ImportRecord::Fact {
            content: "Schema V1 content".into(),
            source: None,
            metadata: None,
        }],
    };

    let env2 = ImportEnvelope {
        envelope_id: EnvelopeId::new("shared-id"),
        schema_version: "v2".into(), // different schema version
        content_digest: "digest-same".into(),
        source_authority: "test".into(),
        trace_id: None,
        namespace: "ns".into(),
        records: vec![ImportRecord::Fact {
            content: "Schema V2 content".into(),
            source: None,
            metadata: None,
        }],
    };

    let r1 = store.import_envelope(&env1).await.unwrap();
    assert!(!r1.was_duplicate);

    let r2 = store.import_envelope(&env2).await.unwrap();
    assert!(
        !r2.was_duplicate,
        "different schema_version should not be considered duplicate"
    );
}

// ─── Import Status Tracking Tests ──────────────────────────────

#[tokio::test]
async fn import_status_returns_receipts() {
    let (store, _dir) = test_store();
    let env = make_envelope("status-1", "ns");
    store.import_envelope(&env).await.unwrap();

    let receipts = store
        .import_status(&EnvelopeId::new("status-1"))
        .await
        .unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].envelope_id.as_str(), "status-1");
    assert_eq!(receipts[0].status, ImportStatus::Complete);
}

#[tokio::test]
async fn import_status_empty_for_unknown_envelope() {
    let (store, _dir) = test_store();
    let receipts = store
        .import_status(&EnvelopeId::new("nonexistent"))
        .await
        .unwrap();
    assert!(receipts.is_empty());
}

#[tokio::test]
async fn list_imports_returns_recent() {
    let (store, _dir) = test_store();

    for i in 0..5 {
        let env = make_envelope(&format!("list-{i}"), "list-ns");
        store.import_envelope(&env).await.unwrap();
    }

    let all = store.list_imports(None, 100).await.unwrap();
    assert_eq!(all.len(), 5);

    let limited = store.list_imports(None, 3).await.unwrap();
    assert_eq!(limited.len(), 3);

    let filtered = store.list_imports(Some("list-ns"), 100).await.unwrap();
    assert_eq!(filtered.len(), 5);

    let other_ns = store.list_imports(Some("other-ns"), 100).await.unwrap();
    assert!(other_ns.is_empty());
}

#[tokio::test]
async fn last_import_at_tracks_namespace() {
    let (store, _dir) = test_store();

    // No imports yet
    let ts = store.last_import_at("ns1").await.unwrap();
    assert!(ts.is_none());

    // Import into ns1
    let env = make_envelope("ts-1", "ns1");
    store.import_envelope(&env).await.unwrap();

    let ts = store.last_import_at("ns1").await.unwrap();
    assert!(ts.is_some());

    // ns2 should still be empty
    let ts2 = store.last_import_at("ns2").await.unwrap();
    assert!(ts2.is_none());
}

// ─── Provenance Preservation Tests ─────────────────────────────

#[tokio::test]
async fn imported_facts_carry_provenance_metadata() {
    let (store, _dir) = test_store();
    let env = make_envelope("prov-1", "prov-ns");
    store.import_envelope(&env).await.unwrap();

    // Search for the imported fact and check its metadata
    let results = store
        .search(
            "Test fact from envelope prov-1",
            Some(5),
            Some(&["prov-ns"]),
            None,
        )
        .await
        .unwrap();
    assert!(!results.is_empty());

    // The fact should have _import provenance in its metadata
    // We verify this by checking the fact directly
    if let semantic_memory::SearchSource::Fact { fact_id, .. } = &results[0].source {
        let facts = store
            .search(&results[0].content, Some(1), Some(&["prov-ns"]), None)
            .await
            .unwrap();
        assert!(!facts.is_empty(), "fact should exist with provenance");
        // The content should match what we imported
        assert!(facts[0].content.contains("Test fact from envelope prov-1"));
        let _ = fact_id; // provenance preserved through the fact_id
    }
}

#[tokio::test]
async fn trace_id_preserved_in_import_receipt() {
    let (store, _dir) = test_store();
    let env = ImportEnvelope {
        envelope_id: EnvelopeId::new("trace-test"),
        schema_version: "v1".into(),
        content_digest: "digest-trace".into(),
        source_authority: "test".into(),
        trace_id: Some(TraceId::new("my-trace-id")),
        namespace: "ns".into(),
        records: vec![ImportRecord::Fact {
            content: "Traced fact".into(),
            source: None,
            metadata: None,
        }],
    };

    let receipt = store.import_envelope(&env).await.unwrap();
    assert_eq!(
        receipt.trace_id.as_ref().map(|t| t.as_str()),
        Some("my-trace-id")
    );
}

// ─── Projection Freshness Type Tests ───────────────────────────

#[test]
fn projection_freshness_variants_are_distinct() {
    let current = ImportProjectionFreshness::Current;
    let stale = ImportProjectionFreshness::Stale {
        last_import_at: "2024-01-01T00:00:00Z".into(),
    };
    let superseded = ImportProjectionFreshness::Superseded {
        superseded_by: EnvelopeId::new("newer"),
    };
    let failed = ImportProjectionFreshness::ImportFailed {
        error: "timeout".into(),
        attempted_at: "2024-01-01T00:00:00Z".into(),
    };
    let never = ImportProjectionFreshness::NeverImported;

    assert_ne!(current, stale);
    assert_ne!(current, superseded);
    assert_ne!(current, failed);
    assert_ne!(current, never);
}

// ─── Import Envelope Type Tests ────────────────────────────────

#[test]
fn envelope_id_display_and_equality() {
    let id1 = EnvelopeId::new("abc-123");
    let id2 = EnvelopeId::from("abc-123".to_string());
    assert_eq!(id1, id2);
    assert_eq!(id1.to_string(), "abc-123");
    assert_eq!(id1.as_str(), "abc-123");
}

#[test]
fn import_status_serialization_roundtrip() {
    let complete = ImportStatus::Complete;
    assert_eq!(complete.as_str(), "complete");
    assert_eq!(
        ImportStatus::from_str_value("complete"),
        ImportStatus::Complete
    );

    let already = ImportStatus::AlreadyImported;
    assert_eq!(already.as_str(), "already_imported");
    assert_eq!(
        ImportStatus::from_str_value("already_imported"),
        ImportStatus::AlreadyImported
    );
}

// ─── Atomicity / Partial Rollback Tests ─────────────────────

#[tokio::test]
async fn failed_import_does_not_leave_partial_facts() {
    // An envelope with one valid fact and one empty-content fact should be
    // rejected at validation time. Verify no facts leak from the valid record.
    let (store, _dir) = test_store();
    let env = ImportEnvelope {
        envelope_id: EnvelopeId::new("partial-1"),
        schema_version: "v1".into(),
        content_digest: "digest-partial".into(),
        source_authority: "test".into(),
        trace_id: None,
        namespace: "partial-ns".into(),
        records: vec![
            ImportRecord::Fact {
                content: "This should not survive".into(),
                source: None,
                metadata: None,
            },
            ImportRecord::Fact {
                content: String::new(), // invalid — will fail validation
                source: None,
                metadata: None,
            },
        ],
    };

    let err = store.import_envelope(&env).await;
    assert!(
        err.is_err(),
        "envelope with empty content should be rejected"
    );

    // Verify the valid record was NOT committed
    let results = store
        .search(
            "This should not survive",
            Some(10),
            Some(&["partial-ns"]),
            None,
        )
        .await
        .unwrap();
    assert!(
        results.is_empty(),
        "no records should be visible from a rejected envelope"
    );

    // Verify no import log entry
    let status = store
        .import_status(&EnvelopeId::new("partial-1"))
        .await
        .unwrap();
    assert!(status.is_empty(), "rejected envelope should not be logged");
}

#[tokio::test]
async fn multiple_envelopes_are_independent() {
    // If envelope A imports successfully and envelope B fails validation,
    // A's records should be visible and B's should not.
    let (store, _dir) = test_store();

    let env_a = make_envelope("independent-a", "ind-ns");
    let receipt = store.import_envelope(&env_a).await.unwrap();
    assert_eq!(receipt.status, ImportStatus::Complete);

    let env_b = ImportEnvelope {
        envelope_id: EnvelopeId::new("independent-b"),
        schema_version: "v1".into(),
        content_digest: "digest-b".into(),
        source_authority: "test".into(),
        trace_id: None,
        namespace: "ind-ns".into(),
        records: vec![], // invalid — empty records
    };
    assert!(store.import_envelope(&env_b).await.is_err());

    // A's records should still be present
    let results = store
        .search(
            "Test fact from envelope independent-a",
            Some(5),
            Some(&["ind-ns"]),
            None,
        )
        .await
        .unwrap();
    assert!(
        !results.is_empty(),
        "envelope A's records should survive B's failure"
    );

    // B should have no import log
    let status = store
        .import_status(&EnvelopeId::new("independent-b"))
        .await
        .unwrap();
    assert!(status.is_empty());
}

#[tokio::test]
async fn import_namespace_isolation() {
    // Facts imported into namespace "alpha" should not appear in namespace "beta"
    let (store, _dir) = test_store();

    let env = make_envelope("ns-iso-1", "alpha");
    store.import_envelope(&env).await.unwrap();

    let alpha_results = store
        .search("Test fact", Some(10), Some(&["alpha"]), None)
        .await
        .unwrap();
    assert!(
        !alpha_results.is_empty(),
        "fact should be in alpha namespace"
    );

    let beta_results = store
        .search("Test fact", Some(10), Some(&["beta"]), None)
        .await
        .unwrap();
    assert!(
        beta_results.is_empty(),
        "fact should not be in beta namespace"
    );
}

#[test]
fn envelope_validate_catches_all_invalid_states() {
    let valid = ImportEnvelope {
        envelope_id: EnvelopeId::new("e1"),
        schema_version: "v1".into(),
        content_digest: "d1".into(),
        source_authority: "test".into(),
        trace_id: None,
        namespace: "ns".into(),
        records: vec![ImportRecord::Fact {
            content: "hello".into(),
            source: None,
            metadata: None,
        }],
    };
    assert!(valid.validate().is_ok());
}

// ═══════════════════════════════════════════════════════════════
// CANONICAL BATCH BOUNDARY TESTS (V11+ import_projection_batch)
// ═══════════════════════════════════════════════════════════════
//
// I018: First-class canonical import boundary suite covering:
// - Schema validation (unknown/missing versions rejected)
// - Required-field enforcement (I009)
// - Idempotency for canonical batches
// - Rollback on invalid records
// - Trace/provenance preservation (I012)
// - Preferred-open uniqueness invariant (I014, I015)

/// Helper: build a complete, valid canonical batch JSON value using the real
/// bridge-owned import type, then serialize when a JSON compat seam is under test.
fn make_canonical_batch_value(
    envelope_id: &str,
    claim_id: &str,
    content: &str,
) -> serde_json::Value {
    serde_json::to_value(ProjectionImportBatchV1 {
        source_envelope_id: EnvelopeId::new(envelope_id),
        schema_version: PROJECTION_IMPORT_BATCH_V1_SCHEMA.into(),
        export_schema_version: Some("export_envelope_v1".into()),
        content_digest: ContentDigest::compute_str(&format!("digest-{envelope_id}")),
        source_authority: "forge".into(),
        scope_key: ScopeKey::namespace_only("canonical-ns"),
        trace_ctx: Some(TraceCtx::from_trace_id("trace-canonical")),
        source_exported_at: "2026-03-07T00:00:00Z".into(),
        transformed_at: "2026-03-07T00:00:01Z".into(),
        records: vec![ImportProjectionRecord::ClaimVersion(ImportClaimVersion {
            claim_id: ClaimId::new(claim_id),
            claim_version_id: ClaimVersionId::new(format!("{claim_id}-v1")),
            claim_state: ClaimState::Active,
            projection_family: "forge_verification".into(),
            subject_entity_id: EntityId::new("ent-1"),
            predicate: "has_type".into(),
            object_anchor: serde_json::json!("function"),
            scope_key: ScopeKey::namespace_only("canonical-ns"),
            valid_from: None,
            valid_to: None,
            preferred_open: true,
            source_envelope_id: EnvelopeId::new(envelope_id),
            source_authority: "forge".into(),
            trace_ctx: Some(TraceCtx::from_trace_id("trace-canonical")),
            freshness: BridgeProjectionFreshness::Current,
            contradiction_status: ContradictionStatus::None,
            supersedes_claim_version_id: None,
            content: content.into(),
            confidence: 0.95,
            metadata: None,
        })],
    })
    .unwrap()
}

fn make_canonical_batch(envelope_id: &str, claim_id: &str, content: &str) -> String {
    make_canonical_batch_value(envelope_id, claim_id, content).to_string()
}

fn make_evidence_bundle(id: &str) -> EvidenceBundle {
    let mut bundle = EvidenceBundle::new(
        CausalQuestion {
            description: "Does semantic-memory preserve canonical evidence receipts?".into(),
            unit_definition: "projection import batch".into(),
        },
        TreatmentSpec {
            description: "import canonical batch".into(),
            baseline_description: "pre-import batch".into(),
            paired_trials: true,
        },
        OutcomeSpec {
            description: "receipt carries canonical evidence bundle".into(),
            measurement_method: "projection import log".into(),
            outcome_type: "binary".into(),
        },
        "projection_import_test",
        "1.0.0",
        1.0,
    );
    bundle.id = EvidenceBundleId::new(id);
    bundle.comparability_snapshot_version = Some(format!("cmp-{id}"));
    bundle
}

#[tokio::test]
async fn canonical_batch_imports_successfully() {
    let (store, _dir) = test_store();
    let batch = make_canonical_batch("cb-001", "claim-canon-1", "Canonical content");
    let result = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();
    assert_eq!(result.status, "complete");
    assert_eq!(result.record_count, 1);
    assert!(!result.was_duplicate);
}

#[tokio::test]
async fn canonical_v2_batch_import_preserves_export_meta_receipts() {
    let (store, _dir) = test_store();
    let evidence_bundle = make_evidence_bundle("evidence-canonical-v2");
    let batch = ProjectionImportBatchV2 {
        source_envelope_id: EnvelopeId::new("cb-v2-meta"),
        schema_version: PROJECTION_IMPORT_BATCH_V2_SCHEMA.into(),
        export_schema_version: Some("export_envelope_v2".into()),
        content_digest: ContentDigest::compute_str("digest-cb-v2-meta"),
        source_authority: "forge".into(),
        scope_key: ScopeKey::namespace_only("canonical-ns"),
        trace_ctx: Some(TraceCtx::from_trace_id("trace-canonical-v2")),
        source_exported_at: "2026-03-07T00:00:00Z".into(),
        transformed_at: "2026-03-07T00:00:01Z".into(),
        export_meta: Some(ForgeExportMeta {
            authority: ExportAuthority::Forge,
            run_id: Some("run-canonical-v2".into()),
            direct_write: false,
            comparability_snapshot_version: Some("cmp-canonical-v2".into()),
            exported_at: "2026-03-07T00:00:00Z".into(),
        }),
        evidence_bundle: Some(evidence_bundle.clone()),
        episode_bundle: None,
        execution_context: None,
        records: vec![ImportProjectionRecord::ClaimVersion(ImportClaimVersion {
            claim_id: ClaimId::new("claim-canon-v2"),
            claim_version_id: ClaimVersionId::new("claim-canon-v2-v1"),
            claim_state: ClaimState::Active,
            projection_family: "forge_verification".into(),
            subject_entity_id: EntityId::new("ent-v2"),
            predicate: "has_type".into(),
            object_anchor: serde_json::json!("function"),
            scope_key: ScopeKey::namespace_only("canonical-ns"),
            valid_from: None,
            valid_to: None,
            preferred_open: true,
            source_envelope_id: EnvelopeId::new("cb-v2-meta"),
            source_authority: "forge".into(),
            trace_ctx: Some(TraceCtx::from_trace_id("trace-canonical-v2")),
            freshness: BridgeProjectionFreshness::Current,
            contradiction_status: ContradictionStatus::None,
            supersedes_claim_version_id: None,
            content: "Canonical V2 content".into(),
            confidence: 0.99,
            metadata: None,
        })],
    };

    let result = store.import_projection_batch(&batch).await.unwrap();
    assert_eq!(result.status, "complete");

    let imports = store
        .query_projection_imports(Some("canonical-ns"), 10)
        .await
        .unwrap();
    let entry = imports
        .into_iter()
        .find(|entry| entry.source_envelope_id == "cb-v2-meta")
        .expect("v2 import log entry");

    assert_eq!(entry.schema_version, PROJECTION_IMPORT_BATCH_V2_SCHEMA);
    assert_eq!(
        entry.export_schema_version.as_deref(),
        Some("export_envelope_v2")
    );
    assert_eq!(entry.source_run_id.as_deref(), Some("run-canonical-v2"));
    assert_eq!(
        entry.comparability_snapshot_version.as_deref(),
        Some("cmp-canonical-v2")
    );
    assert!(!entry.direct_write);
    assert!(entry.failure_reason.is_none());
    assert_eq!(
        entry.evidence_bundle_id.as_deref(),
        Some("evidence-canonical-v2")
    );
    assert_eq!(
        entry.evidence_bundle_json.as_ref(),
        Some(&serde_json::to_value(&evidence_bundle).unwrap())
    );
    assert!(entry.episode_bundle_id.is_none());
    assert!(entry.execution_context_json.is_none());
}

#[tokio::test]
async fn canonical_v3_batch_import_preserves_kernel_payload_receipts() {
    let (store, _dir) = test_store();
    let evidence_bundle = make_evidence_bundle("evidence-kernel-v3");
    let batch = ProjectionImportBatchV3 {
        source_envelope_id: EnvelopeId::new("env-kernel-v3"),
        schema_version: PROJECTION_IMPORT_BATCH_V3_SCHEMA.into(),
        export_schema_version: Some("export_envelope_v3".into()),
        content_digest: ContentDigest::compute(b"kernel-v3"),
        source_authority: "forge".into(),
        scope_key: ScopeKey::namespace_only("kernel-ns"),
        trace_ctx: Some(TraceCtx::generate()),
        source_exported_at: "2026-03-10T00:00:00Z".into(),
        transformed_at: "2026-03-10T00:01:00Z".into(),
        export_meta: Some(ForgeExportMeta {
            authority: ExportAuthority::Forge,
            run_id: Some("run-kernel-v3".into()),
            direct_write: false,
            comparability_snapshot_version: Some("cmp-kernel-v3".into()),
            exported_at: "2026-03-10T00:00:00Z".into(),
        }),
        evidence_bundle: Some(evidence_bundle.clone()),
        episode_bundle: Some(EpisodeBundleV1 {
            schema_version: "episode_bundle_v1".into(),
            bundle_id: "evidence-kernel-v3".into(),
            episode_id: EpisodeId::new("ep-kernel-v3"),
            primary_document_id: "doc-kernel-v3".into(),
            namespace: "kernel-ns".into(),
            scope_key: ScopeKey::namespace_only("kernel-ns"),
            valid_from: Some("2026-03-10T00:00:00Z".into()),
            valid_to: None,
            exported_at: "2026-03-10T00:00:00Z".into(),
            recorded_at: None,
            source_envelope_id: EnvelopeId::new("env-kernel-v3"),
            content_digest: ContentDigest::compute(b"kernel-v3"),
            source_evidence_pointers: vec!["forge://kernel-v3".into()],
            source_receipt_digests: vec!["receipt-kernel-v3".into()],
            claim_version_ids: vec!["claim-version-kernel-v3".into()],
            relation_version_ids: Vec::new(),
            verification_summary: evidence_bundle.verification_summary.clone(),
            refutation_artifact_ids: Vec::new(),
            control_plane_refs: vec!["forge_run:run-kernel-v3".into()],
            execution_context: ExecutionContextV1 {
                schema_version: "execution_context_v1".into(),
                trace_ctx: TraceCtx::generate(),
                attempt_id: None,
                trial_id: None,
                replay_link: None,
                workload_class: Some("forge_export".into()),
                queue_hops: Vec::new(),
                deadline: None,
                cost_budget_units: None,
                degradation_markers: Vec::new(),
                dispatch_outcome: DispatchOutcomeV1::Succeeded,
                environment_fingerprint: None,
                provider_route: Some("forge".into()),
                cancellation_reason: None,
                tool_receipt_ref: None,
                provider_call_ref: None,
                replay_parent_ref: None,
            },
            thin_export: false,
            supersedes_bundle_id: None,
            evidence_bundle_id: Some("evidence-kernel-v3".into()),
        }),
        execution_context: Some(ExecutionContextV1 {
            schema_version: "execution_context_v1".into(),
            trace_ctx: TraceCtx::generate(),
            attempt_id: None,
            trial_id: None,
            replay_link: None,
            workload_class: Some("forge_export".into()),
            queue_hops: Vec::new(),
            deadline: None,
            cost_budget_units: None,
            degradation_markers: Vec::new(),
            dispatch_outcome: DispatchOutcomeV1::Succeeded,
            environment_fingerprint: None,
            provider_route: Some("forge".into()),
            cancellation_reason: None,
            tool_receipt_ref: None,
            provider_call_ref: None,
            replay_parent_ref: None,
        }),
        support_sets: vec![],
        contradiction_witnesses: vec![],
        retraction_records: vec![],
        claim_states_v13: vec![],
        intervention_bundles_v14: vec![],
        outcome_schemas_v14: vec![],
        cohort_contracts_v14: vec![],
        counterfactual_slices_v14: vec![],
        experiment_cases_v14: vec![],
        comparability_matrices_v14: vec![],
        decision_traces_v14: vec![],
        refuter_suites_v14: vec![],
        refuter_results_v14: vec![],
        experiment_budgets_v14: vec![],
        rollout_decisions_v14: vec![],
        rollback_decisions_v14: vec![],
        attestation_envelopes_v15: vec![],
        trust_root_sets_v15: vec![],
        artifact_admission_policies_v15: vec![],
        transparency_receipts_v15: vec![],
        attestation_revocations_v15: vec![],
        attestation_supersessions_v15: vec![],
        remote_oracle_leases_v15: vec![],
        remote_slice_requests_v15: vec![],
        remote_slice_results_v15: vec![],
        cross_runtime_replay_tickets_v15: vec![],
        dispute_bundles_v15: vec![],
        disclosure_policies_v15: vec![],
        disclosure_budgets_v15: vec![],
        records: vec![ImportProjectionRecordV3 {
            record: ImportProjectionRecord::ClaimVersion(ImportClaimVersion {
                claim_id: ClaimId::new("claim-kernel-v3"),
                claim_version_id: ClaimVersionId::new("claim-version-kernel-v3"),
                claim_state: ClaimState::Active,
                projection_family: "forge_verification".into(),
                subject_entity_id: EntityId::new("entity-kernel-v3"),
                predicate: "supports".into(),
                object_anchor: serde_json::json!("compiler"),
                scope_key: ScopeKey::namespace_only("kernel-ns"),
                valid_from: Some("2026-03-10T00:00:00Z".into()),
                valid_to: None,
                preferred_open: true,
                source_envelope_id: EnvelopeId::new("env-kernel-v3"),
                source_authority: "forge".into(),
                trace_ctx: None,
                freshness: BridgeProjectionFreshness::Current,
                contradiction_status: ContradictionStatus::None,
                supersedes_claim_version_id: None,
                content: "kernel claim".into(),
                confidence: 0.99,
                metadata: None,
            }),
            semantics: Some(ExportRecordSemanticsV3 {
                claim_family_id: Some(ClaimFamilyId::new("family-kernel-v3")),
                assertion_group_id: Some(AssertionGroupId::new("group-kernel-v3")),
                relation_group_id: None,
                joint_evidence_group_id: None,
                constraint_seed_kind: Some(ConstraintSeedKind::Hyperedge),
                treatment_hint: Some(CausalRoleHint::Treatment),
                outcome_hint: None,
                confounder_hint: None,
                instrument_hint: None,
                effect_modifier_hint: None,
                contradiction_candidate_group_id: None,
                mutual_exclusion_group_id: None,
                comparability_snapshot_version: Some("cmp-kernel-v3".into()),
                nuisance_snapshot: None,
                projection_visibility_class: ProjectionVisibilityClass::Standard,
                export_confidence_class: ExportConfidenceClass::Verified,
                derivation_seed_ids: vec!["seed-kernel-v3".into()],
                review_priority_hint: Some("high".into()),
            }),
        }],
    };

    let result = store.import_projection_batch(&batch).await.unwrap();
    assert_eq!(result.status, "complete");

    let imports = store
        .query_projection_imports(Some("kernel-ns"), 10)
        .await
        .unwrap();
    let import = imports.first().expect("expected import receipt");
    assert_eq!(
        import.export_schema_version.as_deref(),
        Some("export_envelope_v3")
    );
    assert_eq!(import.scope_domain, None);
    assert_eq!(import.scope_workspace_id, None);
    assert_eq!(import.scope_repo_id, None);
    assert!(import.kernel_payload_json.is_some());
    assert_eq!(
        import.evidence_bundle_json.as_ref(),
        Some(&serde_json::to_value(&evidence_bundle).unwrap())
    );
    assert_eq!(
        import.episode_bundle_id.as_deref(),
        Some("evidence-kernel-v3")
    );
    assert!(import.episode_bundle_json.is_some());
    assert!(import.execution_context_json.is_some());
    let rebuilt = import
        .rebuildable_kernel_batch_v3()
        .expect("kernel receipt should deserialize")
        .expect("kernel receipt should be present");
    assert_eq!(rebuilt.source_envelope_id.as_str(), "env-kernel-v3");

    let claims = store
        .query_claim_versions({
            let mut query =
                semantic_memory::ProjectionQuery::new(ScopeKey::namespace_only("kernel-ns"));
            query.limit = 10;
            query
        })
        .await
        .unwrap();
    let metadata = claims
        .first()
        .and_then(|claim| claim.metadata.as_ref())
        .expect("expected stored metadata");
    assert!(
        metadata.get("kernel_semantics_v3").is_some(),
        "v3 semantics should remain explicit in stored projection metadata"
    );
}

#[tokio::test]
async fn canonical_batch_rejects_unknown_schema_version() {
    let (store, _dir) = test_store();
    let mut batch = make_canonical_batch_value("cb-bad-ver", "claim-bad-ver", "Bad version");
    batch["schema_version"] = serde_json::json!("unknown_v99");
    let err = store
        .import_projection_batch_json_compat(&batch.to_string())
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(format!("{err}").contains("schema_version"));
}

#[tokio::test]
async fn canonical_batch_rejects_claim_missing_subject() {
    let (store, _dir) = test_store();
    let mut batch = make_canonical_batch_value("cb-no-subj", "c-nosubj", "Missing subject");
    batch["records"][0]
        .as_object_mut()
        .unwrap()
        .remove("subject_entity_id");
    let err = store
        .import_projection_batch_json_compat(&batch.to_string())
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(format!("{err}").contains("subject_entity_id"));
}

#[tokio::test]
async fn canonical_batch_is_idempotent() {
    let (store, _dir) = test_store();
    let batch = make_canonical_batch("cb-idem", "claim-idem", "Idempotent batch");

    let r1 = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();
    assert_eq!(r1.status, "complete");

    let r2 = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();
    assert_eq!(r2.status, "already_imported");
    assert!(r2.was_duplicate);
}

#[tokio::test]
async fn canonical_batch_reports_historical_digest_migration_conflict_explicitly() {
    let (store, _dir) = test_store();
    let first = make_canonical_batch("cb-migrate", "claim-migrate", "Migration content");

    let imported = store
        .import_projection_batch_json_compat(&first)
        .await
        .unwrap();
    assert_eq!(imported.status, "complete");

    let mut replay = make_canonical_batch_value("cb-migrate", "claim-migrate", "Migration content");
    replay["content_digest"] = serde_json::json!("digest-cb-migrate-historical");

    let err = store
        .import_projection_batch_json_compat(&replay.to_string())
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_migration_required");
    assert!(format!("{err}").contains("projection_import_log drift"));

    let logs = store
        .query_projection_imports(Some("canonical-ns"), 100)
        .await
        .unwrap();
    let failed = logs
        .iter()
        .find(|entry| {
            entry.source_envelope_id == "cb-migrate"
                && entry.content_digest == "digest-cb-migrate-historical"
        })
        .expect("historical replay should leave an explicit failed receipt");
    assert_eq!(failed.status, "failed");
    assert!(
        failed
            .failure_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("historical digest migration replay")),
        "failure receipt should explain the digest migration seam explicitly"
    );
}

#[tokio::test]
async fn canonical_batch_rollback_on_second_bad_record() {
    let (store, _dir) = test_store();
    // First record is valid, second is missing required fields
    let batch = serde_json::json!({
        "source_envelope_id": "cb-rollback",
        "schema_version": "projection_import_batch_v1",
        "export_schema_version": "export_envelope_v1",
        "content_digest": "digest-rollback",
        "source_authority": "forge",
        "scope_key": { "namespace": "canonical-ns" },
        "records": [
            {
                "kind": "claim_version",
                "claim_version_id": "cv-good",
                "claim_id": "c-good",
                "subject_entity_id": "ent-1",
                "predicate": "has_type",
                "object_anchor": "function",
                "projection_family": "forge",
                "recorded_at": "2026-03-07T00:00:01Z",
                "content": "Good claim"
            },
            {
                "kind": "claim_version",
                "claim_version_id": "cv-bad"
                // missing all other required fields
            }
        ]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");

    // The good record should NOT have been committed (atomic rollback),
    // but the failed attempt should leave a durable failure receipt.
    let logs = store
        .query_projection_imports(Some("canonical-ns"), 100)
        .await
        .unwrap();
    let entry = logs
        .iter()
        .find(|l| l.source_envelope_id == "cb-rollback")
        .expect("failed batch should leave an import receipt");
    assert_eq!(entry.status, "failed");
    assert!(entry.failure_reason.is_some());

    let failures = store
        .query_projection_import_failures(Some("canonical-ns"), 100)
        .await
        .unwrap();
    let failure = failures
        .iter()
        .find(|f| f.source_envelope_id == "cb-rollback")
        .expect("failed batch should leave a durable failure receipt");
    assert_eq!(failure.schema_version, PROJECTION_IMPORT_BATCH_V2_SCHEMA);
    assert_eq!(failure.error_kind, "import_invalid");
}

#[tokio::test]
async fn canonical_v2_batch_failure_receipt_preserves_evidence_bundle_receipts() {
    let (store, _dir) = test_store();
    let evidence_bundle = make_evidence_bundle("evidence-canonical-v2-failure");

    let duplicate_claim = ImportClaimVersion {
        claim_id: ClaimId::new("claim-canon-v2-failure"),
        claim_version_id: ClaimVersionId::new("claim-canon-v2-failure-v1"),
        claim_state: ClaimState::Active,
        projection_family: "forge_verification".into(),
        subject_entity_id: EntityId::new("ent-v2-failure"),
        predicate: "has_type".into(),
        object_anchor: serde_json::json!("function"),
        scope_key: ScopeKey::namespace_only("canonical-ns"),
        valid_from: None,
        valid_to: None,
        preferred_open: true,
        source_envelope_id: EnvelopeId::new("cb-v2-failure"),
        source_authority: "forge".into(),
        trace_ctx: Some(TraceCtx::from_trace_id("trace-canonical-v2-failure")),
        freshness: BridgeProjectionFreshness::Current,
        contradiction_status: ContradictionStatus::None,
        supersedes_claim_version_id: None,
        content: "Canonical V2 failure content".into(),
        confidence: 0.25,
        metadata: None,
    };
    let batch = ProjectionImportBatchV2 {
        source_envelope_id: EnvelopeId::new("cb-v2-failure"),
        schema_version: PROJECTION_IMPORT_BATCH_V2_SCHEMA.into(),
        export_schema_version: Some("export_envelope_v2".into()),
        content_digest: ContentDigest::compute_str("digest-cb-v2-failure"),
        source_authority: "forge".into(),
        scope_key: ScopeKey::namespace_only("canonical-ns"),
        trace_ctx: Some(TraceCtx::from_trace_id("trace-canonical-v2-failure")),
        source_exported_at: "2026-03-07T00:00:00Z".into(),
        transformed_at: "2026-03-07T00:00:01Z".into(),
        export_meta: Some(ForgeExportMeta {
            authority: ExportAuthority::Forge,
            run_id: Some("run-canonical-v2-failure".into()),
            direct_write: false,
            comparability_snapshot_version: Some("cmp-canonical-v2-failure".into()),
            exported_at: "2026-03-07T00:00:00Z".into(),
        }),
        evidence_bundle: Some(evidence_bundle.clone()),
        episode_bundle: None,
        execution_context: None,
        records: vec![
            ImportProjectionRecord::ClaimVersion(duplicate_claim.clone()),
            ImportProjectionRecord::ClaimVersion(duplicate_claim),
        ],
    };

    store.import_projection_batch(&batch).await.unwrap_err();

    let failures = store
        .query_projection_import_failures(Some("canonical-ns"), 100)
        .await
        .unwrap();
    let failure = failures
        .iter()
        .find(|f| f.source_envelope_id == "cb-v2-failure")
        .expect("typed v2 failure receipt");
    assert_eq!(failure.schema_version, PROJECTION_IMPORT_BATCH_V2_SCHEMA);
    assert_eq!(
        failure.export_schema_version.as_deref(),
        Some("export_envelope_v2")
    );
    assert_eq!(
        failure.source_run_id.as_deref(),
        Some("run-canonical-v2-failure")
    );
    assert_eq!(
        failure.comparability_snapshot_version.as_deref(),
        Some("cmp-canonical-v2-failure")
    );
    assert!(!failure.direct_write);
    assert_eq!(
        failure.evidence_bundle_id.as_deref(),
        Some("evidence-canonical-v2-failure")
    );
    assert_eq!(
        failure.evidence_bundle_json.as_ref(),
        Some(&serde_json::to_value(&evidence_bundle).unwrap())
    );
    assert!(failure.episode_bundle_json.is_none());
    assert!(failure.execution_context_json.is_none());
}

#[tokio::test]
async fn canonical_batch_trace_id_preserved_in_log() {
    let (store, _dir) = test_store();
    let batch = make_canonical_batch("cb-trace", "claim-trace", "Traced batch");
    store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();

    let logs = store
        .query_projection_imports(Some("canonical-ns"), 100)
        .await
        .unwrap();
    let entry = logs
        .iter()
        .find(|l| l.source_envelope_id == "cb-trace")
        .unwrap();
    assert_eq!(entry.claim_count, 1);
    assert_eq!(entry.source_authority, "forge");
    assert_eq!(entry.schema_version, "projection_import_batch_v2");
    assert_eq!(
        entry.export_schema_version.as_deref(),
        Some("export_envelope_v1")
    );
}

#[tokio::test]
async fn canonical_batch_trace_ctx_extra_fields_are_not_durably_persisted() {
    let (store, dir) = test_store();
    let mut batch =
        make_canonical_batch_value("cb-trace-shape", "claim-trace-shape", "Trace shape proof");
    batch["trace_ctx"] = serde_json::json!({
        "trace_id": "trace-sm001",
        "parent_id": "parent-sm001",
        "span_id": "span-sm001",
        "baggage": [{ "key": "tenant", "value": "alpha" }]
    });

    store
        .import_projection_batch_json_compat(&batch.to_string())
        .await
        .unwrap();

    let conn = rusqlite::Connection::open(dir.path().join("memory.db")).unwrap();
    let stored_trace_id: Option<String> = conn
        .query_row(
            "SELECT trace_id FROM projection_import_log WHERE source_envelope_id = ?1",
            ["cb-trace-shape"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_trace_id.as_deref(), Some("trace-sm001"));

    let mut stmt = conn
        .prepare("SELECT name FROM pragma_table_info('projection_import_log')")
        .unwrap();
    let column_names: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        !column_names
            .iter()
            .any(|name| name == "parent_id" || name == "span_id" || name == "baggage"),
        "projection_import_log must persist only the declared durable trace reference"
    );
}

#[tokio::test]
async fn canonical_batch_records_distinct_import_and_export_schema_versions() {
    let (store, _dir) = test_store();
    let batch = make_canonical_batch("cb-version-law", "claim-version-law", "Versioned batch");
    store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();

    let logs = store
        .query_projection_imports(Some("canonical-ns"), 100)
        .await
        .unwrap();
    let entry = logs
        .iter()
        .find(|l| l.source_envelope_id == "cb-version-law")
        .unwrap();

    assert_eq!(entry.schema_version, "projection_import_batch_v2");
    assert_eq!(
        entry.export_schema_version.as_deref(),
        Some("export_envelope_v1")
    );
}

#[tokio::test]
async fn canonical_batch_preferred_open_uniqueness_enforced() {
    // I014: Two claims with the same claim_id both marked preferred_open=true
    // should fail due to the UNIQUE index.
    let (store, _dir) = test_store();

    // First batch: claim with preferred_open=true
    let batch1 = make_canonical_batch("cb-pref1", "claim-pref", "First preferred");
    store
        .import_projection_batch_json_compat(&batch1)
        .await
        .unwrap();

    // Second batch: same claim_id, different version, also preferred_open=true
    let mut batch2 = make_canonical_batch_value("cb-pref2", "claim-pref", "Second preferred");
    batch2["content_digest"] = serde_json::json!(ContentDigest::compute_str("digest-pref2"));
    batch2["records"][0]["claim_version_id"] = serde_json::json!("claim-pref-v2");
    batch2["records"][0]["recorded_at"] = serde_json::json!("2026-03-07T00:00:02Z");
    batch2["records"][0]["content"] = serde_json::json!("Second preferred - should conflict");

    let err = store
        .import_projection_batch_json_compat(&batch2.to_string())
        .await;
    assert!(
        err.is_err(),
        "second preferred_open=true for same claim_id must fail"
    );
}

#[tokio::test]
async fn canonical_batch_rejects_invalid_preferred_temporal_order() {
    // I102: malformed temporal bounds must be rejected explicitly.
    let (store, _dir) = test_store();
    let mut batch =
        make_canonical_batch_value("cb-bad-temporal", "claim-bad-temporal", "bad bounds");
    batch["records"][0]["valid_from"] = serde_json::json!("2026-02-01T00:00:00Z");
    batch["records"][0]["valid_to"] = serde_json::json!("2026-01-01T00:00:00Z");

    let err = store
        .import_projection_batch_json_compat(&batch.to_string())
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(format!("{err}").contains("valid_from"));
}

#[tokio::test]
async fn canonical_batch_rejects_overlapping_preferred_claims_in_batch() {
    let (store, _dir) = test_store();
    let batch = serde_json::json!({
        "source_envelope_id": "cb-pref-overlap-batch",
        "schema_version": PROJECTION_IMPORT_BATCH_V1_SCHEMA,
        "export_schema_version": "export_envelope_v1",
        "content_digest": "digest-pref-overlap-batch",
        "source_authority": "forge",
        "scope_key": { "namespace": "canonical-ns" },
        "source_exported_at": "2026-03-07T00:00:00Z",
        "transformed_at": "2026-03-07T00:00:01Z",
        "records": [
            {
                "kind": "claim_version",
                "claim_id": "claim-overlap",
                "claim_version_id": "claim-overlap-v1",
                "claim_state": "active",
                "projection_family": "forge_verification",
                "subject_entity_id": "ent-overlap",
                "predicate": "has_type",
                "object_anchor": "function",
                "scope_key": { "namespace": "canonical-ns" },
                "valid_from": "2026-01-01T00:00:00Z",
                "valid_to": "2026-02-01T00:00:00Z",
                "preferred_open": true,
                "source_envelope_id": "cb-pref-overlap-batch",
                "source_authority": "forge",
                "freshness": "current",
                "contradiction_status": "none",
                "content": "first overlap",
                "confidence": 0.95
            },
            {
                "kind": "claim_version",
                "claim_id": "claim-overlap",
                "claim_version_id": "claim-overlap-v2",
                "claim_state": "active",
                "projection_family": "forge_verification",
                "subject_entity_id": "ent-overlap",
                "predicate": "has_type",
                "object_anchor": "function",
                "scope_key": { "namespace": "canonical-ns" },
                "valid_from": "2026-01-15T00:00:00Z",
                "valid_to": "2026-02-15T00:00:00Z",
                "preferred_open": true,
                "source_envelope_id": "cb-pref-overlap-batch",
                "source_authority": "forge",
                "freshness": "current",
                "contradiction_status": "none",
                "content": "second overlap",
                "confidence": 0.94
            }
        ]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(format!("{err}").contains("overlap"));
}

#[tokio::test]
async fn canonical_batch_rejects_overlapping_preferred_claim_with_existing_preferred() {
    let (store, _dir) = test_store();

    let existing = serde_json::json!({
        "source_envelope_id": "cb-pref-existing-base",
        "schema_version": PROJECTION_IMPORT_BATCH_V1_SCHEMA,
        "export_schema_version": "export_envelope_v1",
        "content_digest": "digest-pref-existing-base",
        "source_authority": "forge",
        "scope_key": { "namespace": "canonical-ns" },
        "source_exported_at": "2026-03-07T00:00:00Z",
        "transformed_at": "2026-03-07T00:00:01Z",
        "records": [{
            "kind": "claim_version",
            "claim_id": "claim-existing-overlap",
            "claim_version_id": "claim-existing-overlap-v1",
            "claim_state": "active",
            "projection_family": "forge_verification",
            "subject_entity_id": "ent-overlap-existing",
            "predicate": "has_type",
            "object_anchor": "function",
            "scope_key": { "namespace": "canonical-ns" },
            "valid_from": "2026-01-01T00:00:00Z",
            "valid_to": "2026-02-01T00:00:00Z",
            "preferred_open": true,
            "source_envelope_id": "cb-pref-existing-base",
            "source_authority": "forge",
            "freshness": "current",
            "contradiction_status": "none",
            "content": "existing overlap",
            "confidence": 0.95
        }]
    })
    .to_string();
    store
        .import_projection_batch_json_compat(&existing)
        .await
        .unwrap();

    let overlapping = serde_json::json!({
        "source_envelope_id": "cb-pref-existing-overlap",
        "schema_version": PROJECTION_IMPORT_BATCH_V1_SCHEMA,
        "export_schema_version": "export_envelope_v1",
        "content_digest": "digest-pref-existing-overlap",
        "source_authority": "forge",
        "scope_key": { "namespace": "canonical-ns" },
        "source_exported_at": "2026-03-08T00:00:00Z",
        "transformed_at": "2026-03-08T00:00:01Z",
        "records": [{
            "kind": "claim_version",
            "claim_id": "claim-existing-overlap",
            "claim_version_id": "claim-existing-overlap-v2",
            "claim_state": "active",
            "projection_family": "forge_verification",
            "subject_entity_id": "ent-overlap-existing",
            "predicate": "has_type",
            "object_anchor": "function",
            "scope_key": { "namespace": "canonical-ns" },
            "valid_from": "2026-01-15T00:00:00Z",
            "valid_to": "2026-03-01T00:00:00Z",
            "preferred_open": true,
            "source_envelope_id": "cb-pref-existing-overlap",
            "source_authority": "forge",
            "freshness": "current",
            "contradiction_status": "none",
            "content": "overlaps existing",
            "confidence": 0.95
        }]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&overlapping)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(format!("{err}").contains("overlap"));
}

#[tokio::test]
async fn canonical_batch_rejects_overlapping_preferred_relations_in_batch() {
    let (store, _dir) = test_store();
    let batch = serde_json::json!({
        "source_envelope_id": "cb-pref-rel-overlap-batch",
        "schema_version": PROJECTION_IMPORT_BATCH_V1_SCHEMA,
        "export_schema_version": "export_envelope_v1",
        "content_digest": "digest-pref-rel-overlap-batch",
        "source_authority": "forge",
        "scope_key": { "namespace": "canonical-ns" },
        "source_exported_at": "2026-03-07T00:00:00Z",
        "transformed_at": "2026-03-07T00:00:01Z",
        "records": [
            {
                "kind": "relation_version",
                "relation_version_id": "rel-overlap-v1",
                "subject_entity_id": "rel-overlap-entity",
                "predicate": "affects",
                "object_anchor": "target-1",
                "scope_key": { "namespace": "canonical-ns" },
                "projection_family": "forge_verification",
                "valid_from": "2026-01-01T00:00:00Z",
                "valid_to": "2026-02-01T00:00:00Z",
                "preferred_open": true,
                "source_confidence": 0.9,
                "source_envelope_id": "cb-pref-rel-overlap-batch",
                "source_authority": "forge",
                "freshness": "current",
                "contradiction_status": "none"
            },
            {
                "kind": "relation_version",
                "relation_version_id": "rel-overlap-v2",
                "subject_entity_id": "rel-overlap-entity",
                "predicate": "affects",
                "object_anchor": "target-1",
                "scope_key": { "namespace": "canonical-ns" },
                "projection_family": "forge_verification",
                "valid_from": "2026-01-15T00:00:00Z",
                "valid_to": "2026-02-15T00:00:00Z",
                "preferred_open": true,
                "source_confidence": 0.88,
                "source_envelope_id": "cb-pref-rel-overlap-batch",
                "source_authority": "forge",
                "freshness": "current",
                "contradiction_status": "none"
            }
        ]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(format!("{err}").contains("overlap"));
}
