//! Release-blocking ugly-case tests for projection batch import.
#![allow(clippy::expect_used)]
//!
//! Covers edge cases that could surface in production but are easy to miss:
//!
//! - A1: Duplicate-but-not-identical envelope (same envelope_id, different digest)
//! - A3: Mid-import failure / rollback atomicity (no partial records visible)
//! - A5: Version mismatch (unknown schema_version)
//! - B3: Contradiction import (supersedes chain)
//! - Idempotency (same batch twice yields was_duplicate)
//! - Episode import (episode_links populated, counts correct)

use forge_memory_bridge::PROJECTION_IMPORT_BATCH_V2_SCHEMA;
use semantic_memory::{MemoryConfig, MemoryStore, MockEmbedder};
use tempfile::TempDir;

fn test_store() -> (MemoryStore, TempDir) {
    let dir = TempDir::new().unwrap();
    let config = MemoryConfig {
        base_dir: dir.path().to_path_buf(),
        ..Default::default()
    };
    let store = MemoryStore::open_with_embedder(config, Box::new(MockEmbedder::new(768))).unwrap();
    (store, dir)
}

fn projection_row_recorded_at(
    dir: &TempDir,
    table: &str,
    source_envelope_id: &str,
) -> rusqlite::Result<String> {
    let conn = rusqlite::Connection::open(dir.path().join("memory.db"))?;
    conn.query_row(
        &format!("SELECT recorded_at FROM {table} WHERE source_envelope_id = ?1 LIMIT 1"),
        rusqlite::params![source_envelope_id],
        |row| row.get(0),
    )
}

// ─── A1: Duplicate-but-not-identical envelope ─────────────────

#[tokio::test]
async fn a1_same_envelope_id_different_digest_both_accepted() {
    let (store, _dir) = test_store();

    // First import: envelope_id="env1", content_digest="digest_a"
    let batch_a = serde_json::json!({
        "source_envelope_id": "env1",
        "schema_version": "export_envelope_v1",
        "content_digest": "digest_a",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "trace_ctx": { "trace_id": "trace-a1-1" },
        "source_exported_at": "2024-01-01T00:00:00Z",
        "transformed_at": "2024-01-01T00:00:01Z",
        "records": [{
            "kind": "claim_version",
            "claim_version_id": "cv-a1-a",
            "claim_id": "c-a1-a",
            "claim_state": "active",
            "projection_family": "forge_verification",
            "subject_entity_id": "ent-1",
            "predicate": "has_type",
            "object_anchor": "\"function\"",
            "recorded_at": "2024-01-01T00:00:01Z",
            "freshness": "current",
            "preferred_open": false,
            "contradiction_status": "none",
            "content": "Content version A",
            "confidence": 0.9
        }]
    })
    .to_string();

    let r1 = store
        .import_projection_batch_json_compat(&batch_a)
        .await
        .unwrap();
    assert_eq!(r1.status, "complete");
    assert!(!r1.was_duplicate);

    // Second import: SAME envelope_id, DIFFERENT content_digest
    let batch_b = serde_json::json!({
        "source_envelope_id": "env1",
        "schema_version": "export_envelope_v1",
        "content_digest": "digest_b",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "trace_ctx": { "trace_id": "trace-a1-2" },
        "source_exported_at": "2024-01-01T00:00:00Z",
        "transformed_at": "2024-01-01T00:00:02Z",
        "records": [{
            "kind": "claim_version",
            "claim_version_id": "cv-a1-b",
            "claim_id": "c-a1-b",
            "claim_state": "active",
            "projection_family": "forge_verification",
            "subject_entity_id": "ent-2",
            "predicate": "has_type",
            "object_anchor": "\"module\"",
            "recorded_at": "2024-01-01T00:00:02Z",
            "freshness": "current",
            "preferred_open": false,
            "contradiction_status": "none",
            "content": "Content version B",
            "confidence": 0.85
        }]
    })
    .to_string();

    let r2 = store
        .import_projection_batch_json_compat(&batch_b)
        .await
        .unwrap();
    assert_eq!(
        r2.status, "complete",
        "Different digest should be accepted as a new import"
    );
    assert!(!r2.was_duplicate);

    // Both should appear in the import log
    let logs = store
        .query_projection_imports(Some("test"), 100)
        .await
        .unwrap();
    assert_eq!(
        logs.len(),
        2,
        "Two distinct imports should be recorded for same envelope_id"
    );
}

#[tokio::test]
async fn a1_same_triple_is_idempotent_noop() {
    let (store, _dir) = test_store();

    let batch = serde_json::json!({
        "source_envelope_id": "env1",
        "schema_version": "export_envelope_v1",
        "content_digest": "digest_a",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "trace_ctx": { "trace_id": "trace-idem" },
        "source_exported_at": "2024-01-01T00:00:00Z",
        "transformed_at": "2024-01-01T00:00:01Z",
        "records": [{
            "kind": "claim_version",
            "claim_version_id": "cv-idem-1",
            "claim_id": "c-idem-1",
            "claim_state": "active",
            "projection_family": "forge",
            "subject_entity_id": "ent-1",
            "predicate": "has_type",
            "object_anchor": "\"fn\"",
            "recorded_at": "2024-01-01T00:00:01Z",
            "freshness": "current",
            "preferred_open": false,
            "contradiction_status": "none",
            "content": "Idempotent content",
            "confidence": 0.9
        }]
    })
    .to_string();

    // First import
    let r1 = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();
    assert_eq!(r1.status, "complete");
    assert!(!r1.was_duplicate);

    // Same (envelope_id, schema_version, content_digest) = idempotent no-op
    let r2 = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();
    assert_eq!(r2.status, "already_imported");
    assert!(r2.was_duplicate);
    assert_eq!(
        r2.record_count, 1,
        "Record count should match original batch"
    );

    // Import log should still have exactly one entry
    let logs = store
        .query_projection_imports(Some("test"), 100)
        .await
        .unwrap();
    assert_eq!(
        logs.len(),
        1,
        "Idempotent replay must not create a second log entry"
    );
}

// ─── A3: Mid-import failure / rollback atomicity ──────────────

#[tokio::test]
async fn a3_duplicate_claim_version_id_causes_rollback() {
    let (store, _dir) = test_store();

    // First import: successfully inserts claim_version_id = "cv-dup"
    let batch1 = serde_json::json!({
        "source_envelope_id": "env-rollback-1",
        "schema_version": "export_envelope_v1",
        "content_digest": "digest-rollback-1",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "trace_ctx": { "trace_id": "trace-rb1" },
        "source_exported_at": "2024-01-01T00:00:00Z",
        "transformed_at": "2024-01-01T00:00:01Z",
        "records": [{
            "kind": "claim_version",
            "claim_version_id": "cv-dup",
            "claim_id": "c-dup",
            "claim_state": "active",
            "projection_family": "forge",
            "subject_entity_id": "ent-1",
            "predicate": "has_type",
            "object_anchor": "\"fn\"",
            "recorded_at": "2024-01-01T00:00:01Z",
            "freshness": "current",
            "preferred_open": false,
            "contradiction_status": "none",
            "content": "First claim content",
            "confidence": 0.9
        }]
    })
    .to_string();

    let r1 = store
        .import_projection_batch_json_compat(&batch1)
        .await
        .unwrap();
    assert_eq!(r1.status, "complete");

    // Second import: different envelope, but contains a record with the SAME
    // claim_version_id (PRIMARY KEY). This should fail due to UNIQUE constraint
    // violation. The second valid record in this batch must NOT be committed.
    let batch2 = serde_json::json!({
        "source_envelope_id": "env-rollback-2",
        "schema_version": "export_envelope_v1",
        "content_digest": "digest-rollback-2",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "trace_ctx": { "trace_id": "trace-rb2" },
        "source_exported_at": "2024-01-01T00:00:00Z",
        "transformed_at": "2024-01-01T00:00:02Z",
        "records": [
            {
                "kind": "claim_version",
                "claim_version_id": "cv-dup",
                "claim_id": "c-dup-2",
                "claim_state": "active",
                "projection_family": "forge",
                "subject_entity_id": "ent-2",
                "predicate": "has_type",
                "object_anchor": "\"mod\"",
                "recorded_at": "2024-01-01T00:00:02Z",
                "freshness": "current",
                "preferred_open": false,
                "contradiction_status": "none",
                "content": "Conflicting claim (same PK)",
                "confidence": 0.8
            },
            {
                "kind": "claim_version",
                "claim_version_id": "cv-innocent-bystander",
                "claim_id": "c-bystander",
                "claim_state": "active",
                "projection_family": "forge",
                "subject_entity_id": "ent-3",
                "predicate": "has_type",
                "object_anchor": "\"struct\"",
                "recorded_at": "2024-01-01T00:00:02Z",
                "freshness": "current",
                "preferred_open": false,
                "contradiction_status": "none",
                "content": "Innocent bystander claim",
                "confidence": 0.7
            }
        ]
    })
    .to_string();

    let result = store.import_projection_batch_json_compat(&batch2).await;
    assert!(
        result.is_err(),
        "Import with duplicate claim_version_id PK should fail"
    );

    // Verify no partial records from the failed batch are visible.
    // The import log should NOT have a "complete" entry for env-rollback-2.
    let logs = store
        .query_projection_imports(Some("test"), 100)
        .await
        .unwrap();
    let failed_logs: Vec<_> = logs
        .iter()
        .filter(|l| l.source_envelope_id == "env-rollback-2")
        .collect();
    assert!(
        failed_logs.is_empty() || failed_logs.iter().all(|l| l.status != "complete"),
        "Failed batch must not have a 'complete' import log entry"
    );

    // The first import's data should be untouched
    let first_logs: Vec<_> = logs
        .iter()
        .filter(|l| l.source_envelope_id == "env-rollback-1")
        .collect();
    assert_eq!(
        first_logs.len(),
        1,
        "First import should still be in the log"
    );
    assert_eq!(first_logs[0].status, "complete");
}

// ─── A5: Version mismatch ─────────────────────────────────────

#[tokio::test]
async fn a5_unknown_schema_version_rejected() {
    let (store, _dir) = test_store();

    // Unknown schema_version must be rejected at the import boundary
    // (version-law enforcement). Only known versions are accepted.
    let batch = serde_json::json!({
        "source_envelope_id": "env-schema-unknown",
        "schema_version": "future_schema_v99",
        "content_digest": "digest-schema-unk",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "trace_ctx": { "trace_id": "trace-schema" },
        "source_exported_at": "2024-01-01T00:00:00Z",
        "transformed_at": "2024-01-01T00:00:01Z",
        "records": [{
            "kind": "claim_version",
            "claim_version_id": "cv-schema-1",
            "claim_id": "c-schema-1",
            "claim_state": "active",
            "projection_family": "forge",
            "subject_entity_id": "ent-1",
            "predicate": "has_type",
            "object_anchor": "\"fn\"",
            "recorded_at": "2024-01-01T00:00:01Z",
            "freshness": "current",
            "preferred_open": false,
            "contradiction_status": "none",
            "content": "Claim with unknown schema version",
            "confidence": 0.9
        }]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(
        err.to_string().contains("unsupported schema_version"),
        "Error should mention unsupported schema_version, got: {err}"
    );

    // Rejected compat batches should leave a failed import receipt rather than
    // disappearing before the receipt boundary.
    let logs = store
        .query_projection_imports(Some("test"), 100)
        .await
        .unwrap();
    let log = logs
        .iter()
        .find(|l| l.source_envelope_id == "env-schema-unknown")
        .expect("Rejected batch should leave a failed import receipt");
    assert_eq!(log.status, "failed");
    assert_eq!(log.schema_version, "future_schema_v99");
    assert!(log.failure_reason.is_some());

    let failures = store
        .query_projection_import_failures(Some("test"), 100)
        .await
        .unwrap();
    let failure = failures
        .iter()
        .find(|f| f.source_envelope_id == "env-schema-unknown")
        .expect("Rejected batch should leave a durable failure receipt");
    assert_eq!(failure.schema_version, "future_schema_v99");
    assert_eq!(failure.error_kind, "import_invalid");
}

#[tokio::test]
async fn a5_known_schema_version_accepted() {
    let (store, _dir) = test_store();

    // The legacy export schema token should be accepted at the JSON-compat
    // boundary, then normalized to the canonical bridge batch schema.
    let batch = serde_json::json!({
        "source_envelope_id": "env-schema-known",
        "schema_version": "export_envelope_v1",
        "content_digest": "digest-schema-known",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "trace_ctx": { "trace_id": "trace-schema-ok" },
        "source_exported_at": "2024-01-01T00:00:00Z",
        "transformed_at": "2024-01-01T00:00:01Z",
        "records": [{
            "kind": "claim_version",
            "claim_version_id": "cv-schema-ok",
            "claim_id": "c-schema-ok",
            "claim_state": "active",
            "projection_family": "forge",
            "subject_entity_id": "ent-1",
            "predicate": "has_type",
            "object_anchor": "\"fn\"",
            "recorded_at": "2024-01-01T00:00:01Z",
            "freshness": "current",
            "preferred_open": false,
            "contradiction_status": "none",
            "content": "Claim with known schema version",
            "confidence": 0.9
        }]
    })
    .to_string();

    let result = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();
    assert_eq!(result.status, "complete");
    assert!(!result.was_duplicate);

    let logs = store
        .query_projection_imports(Some("test"), 100)
        .await
        .unwrap();
    let entry = logs
        .iter()
        .find(|l| l.source_envelope_id == "env-schema-known")
        .expect("Import log entry should exist for known schema");
    assert_eq!(entry.schema_version, PROJECTION_IMPORT_BATCH_V2_SCHEMA);
    assert_eq!(
        entry.export_schema_version.as_deref(),
        Some("export_envelope_v1")
    );
}

// ─── B3: Contradiction import (supersedes chain) ──────────────

#[tokio::test]
async fn b3_superseding_claim_both_versions_exist() {
    let (store, _dir) = test_store();

    // First import: original claim with contradiction_status = "none"
    let batch1 = serde_json::json!({
        "source_envelope_id": "env-contra-1",
        "schema_version": "export_envelope_v1",
        "content_digest": "digest-contra-1",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "trace_ctx": { "trace_id": "trace-contra-1" },
        "source_exported_at": "2024-01-01T00:00:00Z",
        "transformed_at": "2024-01-01T00:00:01Z",
        "records": [{
            "kind": "claim_version",
            "claim_version_id": "cv-original",
            "claim_id": "c-contra",
            "claim_state": "active",
            "projection_family": "forge_verification",
            "subject_entity_id": "ent-contra",
            "predicate": "language_of",
            "object_anchor": "\"Rust\"",
            "recorded_at": "2024-01-01T00:00:01Z",
            "freshness": "current",
            "preferred_open": true,
            "contradiction_status": "none",
            "content": "Project X uses Rust",
            "confidence": 0.9
        }]
    })
    .to_string();

    let r1 = store
        .import_projection_batch_json_compat(&batch1)
        .await
        .unwrap();
    assert_eq!(r1.status, "complete");

    // Second import: a superseding claim that references the original
    // via supersedes_claim_version_id. The original claim_state is "superseded".
    let batch2 = serde_json::json!({
        "source_envelope_id": "env-contra-2",
        "schema_version": "export_envelope_v1",
        "content_digest": "digest-contra-2",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "trace_ctx": { "trace_id": "trace-contra-2" },
        "source_exported_at": "2024-01-02T00:00:00Z",
        "transformed_at": "2024-01-02T00:00:01Z",
        "records": [
            {
                "kind": "claim_version",
                "claim_version_id": "cv-original-v2",
                "claim_id": "c-contra",
                "claim_state": "superseded",
                "projection_family": "forge_verification",
                "subject_entity_id": "ent-contra",
                "predicate": "language_of",
                "object_anchor": "\"Rust\"",
                "recorded_at": "2024-01-02T00:00:01Z",
                "freshness": "superseded",
                "preferred_open": false,
                "contradiction_status": "none",
                "supersedes_claim_version_id": "cv-original",
                "content": "Project X uses Rust (superseded)",
                "confidence": 0.9
            },
            {
                "kind": "claim_version",
                "claim_version_id": "cv-replacement",
                "claim_id": "c-contra-new",
                "claim_state": "active",
                "projection_family": "forge_verification",
                "subject_entity_id": "ent-contra",
                "predicate": "language_of",
                "object_anchor": "\"Go\"",
                "recorded_at": "2024-01-02T00:00:01Z",
                "freshness": "current",
                "preferred_open": true,
                "contradiction_status": "none",
                "content": "Project X migrated to Go",
                "confidence": 0.95
            }
        ]
    })
    .to_string();

    let r2 = store
        .import_projection_batch_json_compat(&batch2)
        .await
        .unwrap();
    assert_eq!(r2.status, "complete");
    assert_eq!(r2.record_count, 2);

    // Verify both imports are in the log
    let logs = store
        .query_projection_imports(Some("test"), 100)
        .await
        .unwrap();
    assert_eq!(logs.len(), 2);

    // Verify both claims from the second batch have appropriate counts
    let contra_log = logs
        .iter()
        .find(|l| l.source_envelope_id == "env-contra-2")
        .expect("Second import log should exist");
    assert_eq!(contra_log.claim_count, 2);
}

// ─── Idempotency ──────────────────────────────────────────────

#[tokio::test]
async fn idempotency_second_import_returns_duplicate() {
    let (store, _dir) = test_store();

    let batch = serde_json::json!({
        "source_envelope_id": "env-idempotent",
        "schema_version": "export_envelope_v1",
        "content_digest": "digest-idempotent",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "trace_ctx": { "trace_id": "trace-idem" },
        "source_exported_at": "2024-01-01T00:00:00Z",
        "transformed_at": "2024-01-01T00:00:01Z",
        "records": [
            {
                "kind": "claim_version",
                "claim_version_id": "cv-idem-a",
                "claim_id": "c-idem-a",
                "claim_state": "active",
                "projection_family": "forge",
                "subject_entity_id": "ent-1",
                "predicate": "p1",
                "object_anchor": "\"v1\"",
                "recorded_at": "2024-01-01T00:00:01Z",
                "freshness": "current",
                "preferred_open": false,
                "contradiction_status": "none",
                "content": "Idempotent claim A",
                "confidence": 0.9
            },
            {
                "kind": "relation_version",
                "relation_version_id": "rv-idem-a",
                "subject_entity_id": "ent-1",
                "predicate": "depends_on",
                "object_anchor": "ent-2",
                "recorded_at": "2024-01-01T00:00:01Z",
                "preferred_open": false,
                "source_confidence": 0.8,
                "projection_family": "forge",
                "freshness": "current",
                "contradiction_status": "none"
            }
        ]
    })
    .to_string();

    // First import
    let r1 = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();
    assert_eq!(r1.status, "complete");
    assert!(!r1.was_duplicate);
    assert_eq!(r1.record_count, 2);

    // Second import: exact same batch
    let r2 = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();
    assert_eq!(r2.status, "already_imported");
    assert!(r2.was_duplicate);
    assert_eq!(
        r2.record_count, 2,
        "Record count should reflect original batch"
    );

    // Third import: still idempotent
    let r3 = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();
    assert!(r3.was_duplicate);

    // Import log should have exactly one entry
    let logs = store
        .query_projection_imports(Some("test"), 100)
        .await
        .unwrap();
    let matching: Vec<_> = logs
        .iter()
        .filter(|l| l.source_envelope_id == "env-idempotent")
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "Idempotent re-imports must not create duplicate log entries"
    );
    assert_eq!(matching[0].claim_count, 1);
    assert_eq!(matching[0].relation_count, 1);
}

// ─── Episode import ───────────────────────────────────────────

#[tokio::test]
async fn episode_import_populates_episode_links() {
    let (store, _dir) = test_store();

    let batch = serde_json::json!({
        "source_envelope_id": "env-episode",
        "schema_version": "export_envelope_v1",
        "content_digest": "digest-episode",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "trace_ctx": { "trace_id": "trace-episode" },
        "source_exported_at": "2024-01-01T00:00:00Z",
        "transformed_at": "2024-01-01T00:00:01Z",
        "records": [
            {
                "kind": "episode",
                "episode_id": "ep-1",
                "document_id": "doc-1",
                "cause_ids": ["cause-a", "cause-b"],
                "effect_type": "code_change",
                "outcome": "success",
                "confidence": 0.85,
                "experiment_id": "exp-42",
                "recorded_at": "2024-01-01T00:00:01Z",
                "metadata": { "commit_sha": "abc123" }
            },
            {
                "kind": "episode",
                "episode_id": "ep-2",
                "document_id": "doc-2",
                "cause_ids": [],
                "effect_type": "test_run",
                "outcome": "failure",
                "confidence": 0.6,
                "recorded_at": "2024-01-01T00:00:02Z"
            },
            {
                "kind": "claim_version",
                "claim_version_id": "cv-ep-1",
                "claim_id": "c-ep-1",
                "claim_state": "active",
                "projection_family": "forge",
                "subject_entity_id": "ent-1",
                "predicate": "has_episode",
                "object_anchor": "\"ep-1\"",
                "recorded_at": "2024-01-01T00:00:01Z",
                "freshness": "current",
                "preferred_open": false,
                "contradiction_status": "none",
                "content": "Claim linked to episode",
                "confidence": 0.9
            }
        ]
    })
    .to_string();

    let result = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();
    assert_eq!(result.status, "complete");
    assert_eq!(result.record_count, 3);
    assert!(!result.was_duplicate);

    // Verify import log has correct episode_count
    let logs = store
        .query_projection_imports(Some("test"), 100)
        .await
        .unwrap();
    let entry = logs
        .iter()
        .find(|l| l.source_envelope_id == "env-episode")
        .expect("Episode import log should exist");
    assert_eq!(entry.episode_count, 2, "Two episode records were imported");
    assert_eq!(entry.claim_count, 1, "One claim record was imported");
    assert_eq!(entry.record_count, 3, "Total record count should be 3");
}

// ─── Mixed record types in single batch ───────────────────────

#[tokio::test]
async fn mixed_record_types_all_counted_correctly() {
    let (store, _dir) = test_store();

    let batch = serde_json::json!({
        "source_envelope_id": "env-mixed",
        "schema_version": "export_envelope_v1",
        "content_digest": "digest-mixed",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "trace_ctx": { "trace_id": "trace-mixed" },
        "source_exported_at": "2024-01-01T00:00:00Z",
        "transformed_at": "2024-01-01T00:00:01Z",
        "records": [
            {
                "kind": "claim_version",
                "claim_version_id": "cv-mixed-1",
                "claim_id": "c-mixed-1",
                "claim_state": "active",
                "projection_family": "forge",
                "subject_entity_id": "ent-1",
                "predicate": "has_type",
                "object_anchor": "\"fn\"",
                "recorded_at": "2024-01-01T00:00:01Z",
                "freshness": "current",
                "preferred_open": false,
                "contradiction_status": "none",
                "content": "Mixed claim",
                "confidence": 0.9
            },
            {
                "kind": "claim_version",
                "claim_version_id": "cv-mixed-2",
                "claim_id": "c-mixed-2",
                "claim_state": "active",
                "projection_family": "forge",
                "subject_entity_id": "ent-2",
                "predicate": "has_type",
                "object_anchor": "\"mod\"",
                "recorded_at": "2024-01-01T00:00:01Z",
                "freshness": "current",
                "preferred_open": false,
                "contradiction_status": "none",
                "content": "Another mixed claim",
                "confidence": 0.85
            },
            {
                "kind": "relation_version",
                "relation_version_id": "rv-mixed-1",
                "subject_entity_id": "ent-1",
                "predicate": "depends_on",
                "object_anchor": "ent-2",
                "recorded_at": "2024-01-01T00:00:01Z",
                "preferred_open": true,
                "source_confidence": 0.8,
                "projection_family": "forge",
                "freshness": "current",
                "contradiction_status": "none"
            },
            {
                "kind": "entity_alias",
                "canonical_entity_id": "ent-1",
                "alias_text": "Entity One Alias",
                "alias_source": "forge_extraction",
                "confidence": 0.9,
                "merge_decision": "pending_review",
                "scope": { "namespace": "test" },
                "review_state": "unreviewed",
                "is_human_confirmed": false,
                "is_human_confirmed_final": false,
                "recorded_at": "2024-01-01T00:00:01Z"
            },
            {
                "kind": "evidence_ref",
                "claim_id": "c-mixed-1",
                "claim_version_id": "cv-mixed-1",
                "fetch_handle": "forge://evidence/run-1/artifact-1",
                "source_authority": "forge",
                "recorded_at": "2024-01-01T00:00:01Z"
            },
            {
                "kind": "episode",
                "episode_id": "ep-mixed-1",
                "document_id": "doc-mixed-1",
                "cause_ids": ["cause-1"],
                "effect_type": "code_change",
                "outcome": "success",
                "confidence": 0.7,
                "recorded_at": "2024-01-01T00:00:01Z"
            }
        ]
    })
    .to_string();

    let result = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();
    assert_eq!(result.status, "complete");
    assert_eq!(result.record_count, 6);

    let logs = store
        .query_projection_imports(Some("test"), 100)
        .await
        .unwrap();
    let entry = logs
        .iter()
        .find(|l| l.source_envelope_id == "env-mixed")
        .expect("Mixed import log should exist");
    assert_eq!(entry.claim_count, 2);
    assert_eq!(entry.relation_count, 1);
    assert_eq!(entry.alias_count, 1);
    assert_eq!(entry.evidence_count, 1);
    assert_eq!(entry.episode_count, 1);
    assert_eq!(entry.record_count, 6);
}

// ─── Edge: empty records array ────────────────────────────────

#[tokio::test]
async fn empty_records_array_imports_as_zero_count() {
    let (store, _dir) = test_store();

    let batch = serde_json::json!({
        "source_envelope_id": "env-empty",
        "schema_version": "export_envelope_v1",
        "content_digest": "digest-empty",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "source_exported_at": "2024-01-01T00:00:00Z",
        "transformed_at": "2024-01-01T00:00:01Z",
        "records": []
    })
    .to_string();

    // An empty records array is structurally valid JSON. The batch should
    // either succeed with zero records or be rejected. Either behavior is
    // acceptable, but the result must be deterministic.
    let result = store.import_projection_batch_json_compat(&batch).await;
    match result {
        Ok(r) => {
            assert_eq!(r.record_count, 0);
            assert_eq!(r.status, "complete");
        }
        Err(e) => {
            // If rejected, it should be an import_invalid error, not a crash
            assert_eq!(e.kind(), "import_invalid");
        }
    }
}

// ─── I010: Required-field validation tests ────────────────────
// Replaces the old permissive claim_with_minimal_fields_imports test.
// Claims MUST supply all canonically-required fields.

#[tokio::test]
async fn claim_missing_required_fields_is_rejected() {
    let (store, _dir) = test_store();

    // A claim with only claim_version_id, claim_id, content — missing
    // subject_entity_id, predicate, object_anchor, projection_family, recorded_at.
    let batch = serde_json::json!({
        "source_envelope_id": "env-minimal",
        "schema_version": "export_envelope_v1",
        "content_digest": "digest-minimal",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "source_exported_at": "2024-01-01T00:00:00Z",
        "transformed_at": "2024-01-01T00:00:01Z",
        "records": [{
            "kind": "claim_version",
            "claim_version_id": "cv-minimal",
            "claim_id": "c-minimal",
            "content": "Minimal claim"
        }]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(
        err.kind(),
        "import_invalid",
        "missing required fields must be rejected"
    );
}

#[tokio::test]
async fn claim_with_all_required_fields_imports() {
    let (store, _dir) = test_store();

    // A schema-complete claim with all required fields present.
    let batch = serde_json::json!({
        "source_envelope_id": "env-complete",
        "schema_version": "export_envelope_v1",
        "content_digest": "digest-complete",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "source_exported_at": "2024-01-01T00:00:00Z",
        "transformed_at": "2024-01-01T00:00:01Z",
        "records": [{
            "kind": "claim_version",
            "claim_version_id": "cv-complete",
            "claim_id": "c-complete",
            "claim_state": "active",
            "subject_entity_id": "ent-1",
            "predicate": "has_type",
            "object_anchor": "function",
            "projection_family": "forge_verification",
            "recorded_at": "2024-01-01T00:00:01Z",
            "freshness": "current",
            "preferred_open": false,
            "confidence": 0.9,
            "content": "Complete claim"
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
async fn claim_missing_claim_version_id_is_rejected() {
    let (store, _dir) = test_store();

    let batch = serde_json::json!({
        "source_envelope_id": "env-no-cvid",
        "schema_version": "export_envelope_v1",
        "content_digest": "digest-no-cvid",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "records": [{
            "kind": "claim_version",
            "claim_id": "c-1",
            "subject_entity_id": "ent-1",
            "predicate": "has_type",
            "object_anchor": "function",
            "projection_family": "forge_verification",
            "recorded_at": "2024-01-01T00:00:01Z",
            "content": "No version id"
        }]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(format!("{err}").contains("claim_version_id"));
}

#[tokio::test]
async fn relation_missing_required_fields_is_rejected() {
    let (store, _dir) = test_store();

    let batch = serde_json::json!({
        "source_envelope_id": "env-rv-bad",
        "schema_version": "export_envelope_v1",
        "content_digest": "digest-rv-bad",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "records": [{
            "kind": "relation_version",
            "relation_version_id": "rv-1"
            // missing subject_entity_id, predicate, object_anchor, etc.
        }]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
}

// ─── Edge: episode with empty cause_ids ───────────────────────

#[tokio::test]
async fn episode_with_no_causes_imports() {
    let (store, _dir) = test_store();

    let batch = serde_json::json!({
        "source_envelope_id": "env-ep-nocause",
        "schema_version": "export_envelope_v1",
        "content_digest": "digest-ep-nocause",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "source_exported_at": "2024-01-01T00:00:00Z",
        "transformed_at": "2024-01-01T00:00:01Z",
        "records": [{
            "kind": "episode",
            "episode_id": "ep-nocause",
            "document_id": "doc-nocause",
            "cause_ids": [],
            "effect_type": "observation",
            "outcome": "neutral",
            "confidence": 0.5,
            "recorded_at": "2024-01-01T00:00:01Z"
        }]
    })
    .to_string();

    let result = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();
    assert_eq!(result.status, "complete");
    assert_eq!(result.record_count, 1);

    let logs = store
        .query_projection_imports(Some("test"), 100)
        .await
        .unwrap();
    let entry = logs
        .iter()
        .find(|l| l.source_envelope_id == "env-ep-nocause")
        .unwrap();
    assert_eq!(entry.episode_count, 1);
}

// ─── I006/I007: Semantic default-fill rejection proofs ────────

#[tokio::test]
async fn i007_claim_missing_claim_state_is_rejected() {
    let (store, _dir) = test_store();
    let batch = serde_json::json!({
        "source_envelope_id": "env-no-state",
        "schema_version": "export_envelope_v1",
        "content_digest": "d1",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "records": [{
            "kind": "claim_version",
            "claim_version_id": "cv-1",
            "claim_id": "c-1",
            "subject_entity_id": "ent-1",
            "predicate": "p",
            "object_anchor": "\"v\"",
            "projection_family": "forge",
            "recorded_at": "2024-01-01T00:00:01Z",
            "freshness": "current",
            "preferred_open": false,
            "content": "claim without claim_state"
        }]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(
        format!("{err}").contains("claim_state"),
        "Error should mention claim_state, got: {err}"
    );
}

#[tokio::test]
async fn i007_claim_missing_freshness_is_rejected() {
    let (store, _dir) = test_store();
    let batch = serde_json::json!({
        "source_envelope_id": "env-no-fresh",
        "schema_version": "export_envelope_v1",
        "content_digest": "d2",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "records": [{
            "kind": "claim_version",
            "claim_version_id": "cv-2",
            "claim_id": "c-2",
            "claim_state": "active",
            "subject_entity_id": "ent-1",
            "predicate": "p",
            "object_anchor": "\"v\"",
            "projection_family": "forge",
            "recorded_at": "2024-01-01T00:00:01Z",
            "preferred_open": false,
            "content": "claim without freshness"
        }]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(
        format!("{err}").contains("freshness"),
        "Error should mention freshness, got: {err}"
    );
}

#[tokio::test]
async fn i007_claim_missing_preferred_open_is_rejected() {
    let (store, _dir) = test_store();
    let batch = serde_json::json!({
        "source_envelope_id": "env-no-pref",
        "schema_version": "export_envelope_v1",
        "content_digest": "d3",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "records": [{
            "kind": "claim_version",
            "claim_version_id": "cv-3",
            "claim_id": "c-3",
            "claim_state": "active",
            "subject_entity_id": "ent-1",
            "predicate": "p",
            "object_anchor": "\"v\"",
            "projection_family": "forge",
            "recorded_at": "2024-01-01T00:00:01Z",
            "freshness": "current",
            "content": "claim without preferred_open"
        }]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(
        format!("{err}").contains("preferred_open"),
        "Error should mention preferred_open, got: {err}"
    );
}

#[tokio::test]
async fn i007_relation_missing_freshness_is_rejected() {
    let (store, _dir) = test_store();
    let batch = serde_json::json!({
        "source_envelope_id": "env-rv-no-fresh",
        "schema_version": "export_envelope_v1",
        "content_digest": "d4",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "records": [{
            "kind": "relation_version",
            "relation_version_id": "rv-1",
            "subject_entity_id": "ent-1",
            "predicate": "depends_on",
            "object_anchor": "ent-2",
            "projection_family": "forge",
            "recorded_at": "2024-01-01T00:00:01Z",
            "preferred_open": false,
            "source_confidence": 0.8
        }]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(
        format!("{err}").contains("freshness"),
        "Error should mention freshness, got: {err}"
    );
}

#[tokio::test]
async fn i007_relation_missing_preferred_open_is_rejected() {
    let (store, _dir) = test_store();
    let batch = serde_json::json!({
        "source_envelope_id": "env-rv-no-pref",
        "schema_version": "export_envelope_v1",
        "content_digest": "d5",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "records": [{
            "kind": "relation_version",
            "relation_version_id": "rv-2",
            "subject_entity_id": "ent-1",
            "predicate": "depends_on",
            "object_anchor": "ent-2",
            "projection_family": "forge",
            "recorded_at": "2024-01-01T00:00:01Z",
            "freshness": "current"
        }]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(
        format!("{err}").contains("preferred_open"),
        "Error should mention preferred_open, got: {err}"
    );
}

// ─── I008: importer-stamped recorded_at proofs ────────────────

#[tokio::test]
async fn i008_alias_missing_recorded_at_is_importer_stamped() {
    let (store, dir) = test_store();
    let batch = serde_json::json!({
        "source_envelope_id": "env-alias-no-ts",
        "schema_version": "export_envelope_v1",
        "content_digest": "d6",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "source_exported_at": "2024-01-01T00:00:00Z",
        "transformed_at": "2024-01-01T00:00:01Z",
        "records": [{
            "kind": "entity_alias",
            "canonical_entity_id": "ent-1",
            "alias_text": "alias",
            "alias_source": "forge",
            "confidence": 0.8,
            "merge_decision": "pending_review"
        }]
    })
    .to_string();

    store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();

    let entry = store
        .query_projection_imports(Some("test"), 10)
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.source_envelope_id == "env-alias-no-ts")
        .unwrap();
    let recorded_at =
        projection_row_recorded_at(&dir, "entity_aliases", "env-alias-no-ts").unwrap();
    assert_eq!(recorded_at, entry.imported_at);
    assert_ne!(recorded_at, "2024-01-01T00:00:01Z");
}

#[tokio::test]
async fn i008_evidence_missing_recorded_at_is_importer_stamped() {
    let (store, dir) = test_store();
    let batch = serde_json::json!({
        "source_envelope_id": "env-ev-no-ts",
        "schema_version": "export_envelope_v1",
        "content_digest": "d7",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "source_exported_at": "2024-01-01T00:00:00Z",
        "transformed_at": "2024-01-01T00:00:02Z",
        "records": [{
            "kind": "evidence_ref",
            "claim_id": "c-1",
            "fetch_handle": "forge://evidence/1",
            "source_authority": "forge"
        }]
    })
    .to_string();

    store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();

    let entry = store
        .query_projection_imports(Some("test"), 10)
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.source_envelope_id == "env-ev-no-ts")
        .unwrap();
    let recorded_at = projection_row_recorded_at(&dir, "evidence_refs", "env-ev-no-ts").unwrap();
    assert_eq!(recorded_at, entry.imported_at);
    assert_ne!(recorded_at, "2024-01-01T00:00:02Z");
}

#[tokio::test]
async fn i008_episode_missing_recorded_at_is_importer_stamped() {
    let (store, dir) = test_store();
    let batch = serde_json::json!({
        "source_envelope_id": "env-ep-no-ts",
        "schema_version": "export_envelope_v1",
        "content_digest": "d8",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "source_exported_at": "2024-01-01T00:00:00Z",
        "transformed_at": "2024-01-01T00:00:03Z",
        "records": [{
            "kind": "episode",
            "episode_id": "ep-1",
            "document_id": "doc-1",
            "effect_type": "observation",
            "outcome": "neutral",
            "confidence": 0.5
        }]
    })
    .to_string();

    store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();

    let entry = store
        .query_projection_imports(Some("test"), 10)
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.source_envelope_id == "env-ep-no-ts")
        .unwrap();
    let recorded_at = projection_row_recorded_at(&dir, "episode_links", "env-ep-no-ts").unwrap();
    assert_eq!(recorded_at, entry.imported_at);
    assert_ne!(recorded_at, "2024-01-01T00:00:03Z");
}

// ─── I011: Relation preferred_open scope uniqueness ───────────

#[tokio::test]
async fn i011_different_scope_dimensions_can_coexist_as_preferred_open() {
    let (store, _dir) = test_store();

    // Import relation in scope (ns=test, domain=code, workspace=ws1)
    let batch1 = serde_json::json!({
        "source_envelope_id": "env-scope-1",
        "schema_version": "export_envelope_v1",
        "content_digest": "d-scope-1",
        "source_authority": "forge",
        "scope_key": { "namespace": "test", "domain": "code", "workspace_id": "ws1" },
        "records": [{
            "kind": "relation_version",
            "relation_version_id": "rv-scope-1",
            "subject_entity_id": "ent-1",
            "predicate": "depends_on",
            "object_anchor": "ent-2",
            "projection_family": "forge",
            "recorded_at": "2024-01-01T00:00:01Z",
            "preferred_open": true,
            "freshness": "current",
            "source_confidence": 0.9
        }]
    })
    .to_string();

    let r1 = store
        .import_projection_batch_json_compat(&batch1)
        .await
        .unwrap();
    assert_eq!(r1.status, "complete");

    // Import identical relation in DIFFERENT scope (ns=test, domain=code, workspace=ws2)
    // This should succeed because the full scope key is different.
    let batch2 = serde_json::json!({
        "source_envelope_id": "env-scope-2",
        "schema_version": "export_envelope_v1",
        "content_digest": "d-scope-2",
        "source_authority": "forge",
        "scope_key": { "namespace": "test", "domain": "code", "workspace_id": "ws2" },
        "records": [{
            "kind": "relation_version",
            "relation_version_id": "rv-scope-2",
            "subject_entity_id": "ent-1",
            "predicate": "depends_on",
            "object_anchor": "ent-2",
            "projection_family": "forge",
            "recorded_at": "2024-01-01T00:00:02Z",
            "preferred_open": true,
            "freshness": "current",
            "source_confidence": 0.9
        }]
    })
    .to_string();

    let r2 = store
        .import_projection_batch_json_compat(&batch2)
        .await
        .unwrap();
    assert_eq!(
        r2.status, "complete",
        "Different workspace_id should allow both to be preferred_open"
    );
}

#[tokio::test]
async fn i011_same_full_scope_duplicate_preferred_open_fails() {
    let (store, _dir) = test_store();

    // Import first relation as preferred_open in exact scope
    let batch1 = serde_json::json!({
        "source_envelope_id": "env-dup-pref-1",
        "schema_version": "export_envelope_v1",
        "content_digest": "d-dup-pref-1",
        "source_authority": "forge",
        "scope_key": { "namespace": "test", "domain": "code", "workspace_id": "ws1" },
        "records": [{
            "kind": "relation_version",
            "relation_version_id": "rv-dup-pref-1",
            "subject_entity_id": "ent-1",
            "predicate": "depends_on",
            "object_anchor": "ent-2",
            "projection_family": "forge",
            "recorded_at": "2024-01-01T00:00:01Z",
            "preferred_open": true,
            "freshness": "current",
            "source_confidence": 0.9
        }]
    })
    .to_string();

    let r1 = store
        .import_projection_batch_json_compat(&batch1)
        .await
        .unwrap();
    assert_eq!(r1.status, "complete");

    // Import second relation in SAME full scope, also preferred_open.
    // This should fail due to the unique index constraint.
    let batch2 = serde_json::json!({
        "source_envelope_id": "env-dup-pref-2",
        "schema_version": "export_envelope_v1",
        "content_digest": "d-dup-pref-2",
        "source_authority": "forge",
        "scope_key": { "namespace": "test", "domain": "code", "workspace_id": "ws1" },
        "records": [{
            "kind": "relation_version",
            "relation_version_id": "rv-dup-pref-2",
            "subject_entity_id": "ent-1",
            "predicate": "depends_on",
            "object_anchor": "ent-2",
            "projection_family": "forge",
            "recorded_at": "2024-01-01T00:00:02Z",
            "preferred_open": true,
            "freshness": "current",
            "source_confidence": 0.8
        }]
    })
    .to_string();

    let result = store.import_projection_batch_json_compat(&batch2).await;
    assert!(
        result.is_err(),
        "Duplicate preferred_open in same full scope should fail"
    );
}

// ─── SM-003: Non-core import default hardening proofs ─────────

#[tokio::test]
async fn sm003_alias_missing_confidence_is_rejected() {
    let (store, _dir) = test_store();
    let batch = serde_json::json!({
        "source_envelope_id": "env-sm003-conf",
        "schema_version": "export_envelope_v1",
        "content_digest": "d-sm003-1",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "records": [{
            "kind": "entity_alias",
            "canonical_entity_id": "ent-1",
            "alias_text": "alias",
            "alias_source": "forge",
            "merge_decision": "pending_review",
            "recorded_at": "2024-01-01T00:00:01Z"
        }]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(
        format!("{err}").contains("confidence"),
        "Error should mention confidence, got: {err}"
    );
}

#[tokio::test]
async fn sm003_alias_missing_merge_decision_is_rejected() {
    let (store, _dir) = test_store();
    let batch = serde_json::json!({
        "source_envelope_id": "env-sm003-md",
        "schema_version": "export_envelope_v1",
        "content_digest": "d-sm003-2",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "records": [{
            "kind": "entity_alias",
            "canonical_entity_id": "ent-1",
            "alias_text": "alias",
            "alias_source": "forge",
            "confidence": 0.8,
            "recorded_at": "2024-01-01T00:00:01Z"
        }]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(
        format!("{err}").contains("merge_decision"),
        "Error should mention merge_decision, got: {err}"
    );
}

#[tokio::test]
async fn sm003_evidence_missing_source_authority_is_rejected() {
    let (store, _dir) = test_store();
    let batch = serde_json::json!({
        "source_envelope_id": "env-sm003-sa",
        "schema_version": "export_envelope_v1",
        "content_digest": "d-sm003-3",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "records": [{
            "kind": "evidence_ref",
            "claim_id": "c-1",
            "fetch_handle": "forge://evidence/1",
            "recorded_at": "2024-01-01T00:00:01Z"
        }]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(
        format!("{err}").contains("source_authority"),
        "Error should mention source_authority, got: {err}"
    );
}

#[tokio::test]
async fn sm003_episode_missing_confidence_is_rejected() {
    let (store, _dir) = test_store();
    let batch = serde_json::json!({
        "source_envelope_id": "env-sm003-ec",
        "schema_version": "export_envelope_v1",
        "content_digest": "d-sm003-4",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "records": [{
            "kind": "episode",
            "episode_id": "ep-1",
            "document_id": "doc-1",
            "effect_type": "observation",
            "outcome": "neutral",
            "recorded_at": "2024-01-01T00:00:01Z"
        }]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(
        format!("{err}").contains("confidence"),
        "Error should mention confidence, got: {err}"
    );
}

// SM-003: Prove V1 canonical defaults are accepted (review_state, is_human_confirmed, cause_ids)
#[tokio::test]
async fn sm003_v1_canonical_defaults_accepted() {
    let (store, _dir) = test_store();
    // Alias with only required fields + V1 defaults for optional ones
    let batch = serde_json::json!({
        "source_envelope_id": "env-sm003-defaults",
        "schema_version": "export_envelope_v1",
        "content_digest": "d-sm003-5",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "records": [
            {
                "kind": "entity_alias",
                "canonical_entity_id": "ent-1",
                "alias_text": "Entity One",
                "alias_source": "forge_extraction",
                "confidence": 0.9,
                "merge_decision": "pending_review",
                "recorded_at": "2024-01-01T00:00:01Z"
                // review_state, is_human_confirmed, is_human_confirmed_final all use V1 defaults
            },
            {
                "kind": "episode",
                "episode_id": "ep-defaults",
                "document_id": "doc-1",
                "effect_type": "observation",
                "outcome": "neutral",
                "confidence": 0.5,
                "recorded_at": "2024-01-01T00:00:01Z"
                // cause_ids defaults to []
            }
        ]
    })
    .to_string();

    let result = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();
    assert_eq!(result.status, "complete");
    assert!(!result.was_duplicate);
}

// ─── SM-004: review_state / merge_decision validation proofs ──

#[tokio::test]
async fn sm004_invalid_review_state_is_rejected() {
    let (store, _dir) = test_store();
    let batch = serde_json::json!({
        "source_envelope_id": "env-sm004-rs",
        "schema_version": "export_envelope_v1",
        "content_digest": "d-sm004-1",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "records": [{
            "kind": "entity_alias",
            "canonical_entity_id": "ent-1",
            "alias_text": "alias",
            "alias_source": "forge",
            "confidence": 0.8,
            "merge_decision": "pending_review",
            "review_state": "junk_value",
            "recorded_at": "2024-01-01T00:00:01Z"
        }]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(
        format!("{err}").contains("review_state"),
        "Error should mention review_state, got: {err}"
    );
}

#[tokio::test]
async fn sm004_invalid_merge_decision_is_rejected() {
    let (store, _dir) = test_store();
    let batch = serde_json::json!({
        "source_envelope_id": "env-sm004-md",
        "schema_version": "export_envelope_v1",
        "content_digest": "d-sm004-2",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "records": [{
            "kind": "entity_alias",
            "canonical_entity_id": "ent-1",
            "alias_text": "alias",
            "alias_source": "forge",
            "confidence": 0.8,
            "merge_decision": "not_a_real_decision",
            "recorded_at": "2024-01-01T00:00:01Z"
        }]
    })
    .to_string();

    let err = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "import_invalid");
    assert!(
        format!("{err}").contains("merge_decision"),
        "Error should mention merge_decision, got: {err}"
    );
}

#[tokio::test]
async fn sm004_valid_merge_decision_variants_accepted() {
    let (store, _dir) = test_store();
    // Test all valid merge_decision shapes
    let batch = serde_json::json!({
        "source_envelope_id": "env-sm004-valid",
        "schema_version": "export_envelope_v1",
        "content_digest": "d-sm004-3",
        "source_authority": "forge",
        "scope_key": { "namespace": "test" },
        "records": [
            {
                "kind": "entity_alias",
                "canonical_entity_id": "ent-1",
                "alias_text": "Alias A",
                "alias_source": "forge",
                "confidence": 0.9,
                "merge_decision": "pending_review",
                "recorded_at": "2024-01-01T00:00:01Z"
            },
            {
                "kind": "entity_alias",
                "canonical_entity_id": "ent-2",
                "alias_text": "Alias B",
                "alias_source": "forge",
                "confidence": 0.85,
                "merge_decision": { "automated": { "algorithm": "bridge_default" } },
                "recorded_at": "2024-01-01T00:00:02Z"
            },
            {
                "kind": "entity_alias",
                "canonical_entity_id": "ent-3",
                "alias_text": "Alias C",
                "alias_source": "forge",
                "confidence": 0.95,
                "merge_decision": { "human_reviewed": { "reviewer": "admin", "at": "2024-01-01" } },
                "recorded_at": "2024-01-01T00:00:03Z"
            },
            {
                "kind": "entity_alias",
                "canonical_entity_id": "ent-4",
                "alias_text": "Alias D",
                "alias_source": "forge",
                "confidence": 0.7,
                "merge_decision": { "rejected": { "reason": "false positive" } },
                "recorded_at": "2024-01-01T00:00:04Z"
            }
        ]
    })
    .to_string();

    let result = store
        .import_projection_batch_json_compat(&batch)
        .await
        .unwrap();
    assert_eq!(result.status, "complete");
    assert_eq!(result.record_count, 4);
}
