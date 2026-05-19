//! Projection storage tables and operations for V11+ schema.
//!
//! This module provides the storage layer for claim projection versions,
//! relation versions, entity aliases, evidence refs, and import tracking
//! introduced by the canonical stack v4 architecture.
//!
//! ## Authority
//!
//! `semantic-memory` is authoritative for queryable knowledge state. These
//! projection tables store the imported, queryable representation of data
//! that originated from Forge or other sources via the bridge.
//!
//! ## Validation (SM-004)
//!
//! Entity alias `review_state` and `merge_decision` values are validated at the
//! application level before insertion (see `validate_review_state()` and
//! `validate_merge_decision()` in `lib.rs`). DB-level CHECK constraints exist for
//! claim_state and freshness columns; review_state/merge_decision use application
//! validation because they can contain JSON variant objects.
//!
//! ## Phase status: current / implemented now

use crate::error::MemoryError;

#[path = "projection_storage_query.rs"]
mod projection_storage_query;

pub(crate) use projection_storage_query::*;

/// V11 migration: projection storage tables for canonical stack v4.
///
/// Adds tables for:
/// - `claim_versions` — versioned knowledge assertions with temporal validity
/// - `relation_versions` — versioned entity relations with audit metadata
/// - `entity_aliases` — entity alias/merge state with review durability
/// - `evidence_refs` — opaque evidence references for audit dereference
/// - `projection_import_log` — enhanced import tracking with scope keys
/// - `derivation_edges` — lineage tracking between projections
pub(crate) const MIGRATION_V11: &str = r#"
-- CLAIM PROJECTION VERSIONS
-- Each row is a specific version of a claim. The claim_id is stable across
-- versions; claim_version_id is unique per mutation.
CREATE TABLE IF NOT EXISTS claim_versions (
    claim_version_id        TEXT PRIMARY KEY,
    claim_id                TEXT NOT NULL,
    claim_state             TEXT NOT NULL DEFAULT 'active'
                            CHECK (claim_state IN ('active', 'superseded', 'retracted', 'archived', 'pending_review', 'disputed')),
    projection_family       TEXT NOT NULL,
    subject_entity_id       TEXT NOT NULL,
    predicate               TEXT NOT NULL,
    object_anchor           TEXT NOT NULL,
    scope_namespace         TEXT NOT NULL,
    scope_domain            TEXT,
    scope_workspace_id      TEXT,
    scope_repo_id           TEXT,
    valid_from              TEXT,
    valid_to                TEXT,
    recorded_at             TEXT NOT NULL DEFAULT (datetime('now')),
    preferred_open          INTEGER NOT NULL DEFAULT 0,
    source_envelope_id      TEXT NOT NULL,
    source_authority        TEXT NOT NULL,
    trace_id                TEXT,
    freshness               TEXT NOT NULL DEFAULT 'current'
                            CHECK (freshness IN ('current', 'stale', 'superseded', 'import_failed', 'never_imported', 'import_lagging')),
    contradiction_status    TEXT NOT NULL DEFAULT 'none',
    supersedes_claim_version_id TEXT,
    content                 TEXT NOT NULL,
    confidence              REAL NOT NULL DEFAULT 1.0,
    content_digest          TEXT,
    metadata                TEXT
);

CREATE INDEX IF NOT EXISTS idx_cv_claim_id ON claim_versions(claim_id);
CREATE INDEX IF NOT EXISTS idx_cv_subject ON claim_versions(subject_entity_id);
CREATE INDEX IF NOT EXISTS idx_cv_scope ON claim_versions(scope_namespace, scope_domain);
CREATE INDEX IF NOT EXISTS idx_cv_predicate ON claim_versions(predicate);
CREATE INDEX IF NOT EXISTS idx_cv_state ON claim_versions(claim_state);
-- Enforce at most one preferred_open row per logical claim (I014).
CREATE UNIQUE INDEX IF NOT EXISTS idx_cv_preferred ON claim_versions(claim_id) WHERE preferred_open = 1;
CREATE INDEX IF NOT EXISTS idx_cv_recorded ON claim_versions(recorded_at DESC);
CREATE INDEX IF NOT EXISTS idx_cv_envelope ON claim_versions(source_envelope_id);
CREATE INDEX IF NOT EXISTS idx_cv_freshness ON claim_versions(freshness);

-- RELATION VERSIONS
-- Preserves audit-grade metadata parity with claim versions.
CREATE TABLE IF NOT EXISTS relation_versions (
    relation_version_id     TEXT PRIMARY KEY,
    subject_entity_id       TEXT NOT NULL,
    predicate               TEXT NOT NULL,
    object_anchor           TEXT NOT NULL,
    scope_namespace         TEXT NOT NULL,
    scope_domain            TEXT,
    scope_workspace_id      TEXT,
    scope_repo_id           TEXT,
    claim_id                TEXT,
    source_episode_id       TEXT,
    valid_from              TEXT,
    valid_to                TEXT,
    recorded_at             TEXT NOT NULL DEFAULT (datetime('now')),
    preferred_open          INTEGER NOT NULL DEFAULT 0,
    supersedes_relation_version_id TEXT,
    contradiction_status    TEXT NOT NULL DEFAULT 'none',
    source_confidence       REAL NOT NULL DEFAULT 1.0,
    projection_family       TEXT NOT NULL,
    source_envelope_id      TEXT NOT NULL,
    source_authority        TEXT NOT NULL,
    trace_id                TEXT,
    freshness               TEXT NOT NULL DEFAULT 'current'
                            CHECK (freshness IN ('current', 'stale', 'superseded', 'import_failed', 'never_imported', 'import_lagging')),
    metadata                TEXT
);

CREATE INDEX IF NOT EXISTS idx_rv_subject ON relation_versions(subject_entity_id);
CREATE INDEX IF NOT EXISTS idx_rv_predicate ON relation_versions(predicate);
CREATE INDEX IF NOT EXISTS idx_rv_scope ON relation_versions(scope_namespace, scope_domain);
CREATE INDEX IF NOT EXISTS idx_rv_claim ON relation_versions(claim_id);
CREATE INDEX IF NOT EXISTS idx_rv_episode ON relation_versions(source_episode_id);
CREATE INDEX IF NOT EXISTS idx_rv_envelope ON relation_versions(source_envelope_id);
CREATE INDEX IF NOT EXISTS idx_rv_recorded ON relation_versions(recorded_at DESC);
-- Enforce at most one preferred_open row per logical relation key.
-- V5 spec §11.5: logical key is (subject_entity_id, predicate, object_anchor, scope_key, projection_family).
-- scope_key decomposes to (scope_namespace, scope_domain, scope_workspace_id, scope_repo_id).
-- COALESCE maps NULLs to '' so the uniqueness constraint works correctly.
CREATE UNIQUE INDEX IF NOT EXISTS idx_rv_preferred
    ON relation_versions(
        subject_entity_id, predicate, object_anchor,
        scope_namespace, COALESCE(scope_domain, ''),
        COALESCE(scope_workspace_id, ''), COALESCE(scope_repo_id, ''),
        projection_family
    )
    WHERE preferred_open = 1;

-- ENTITY ALIASES
-- Includes explicit scope semantics and durable review state.
CREATE TABLE IF NOT EXISTS entity_aliases (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    canonical_entity_id     TEXT NOT NULL,
    alias_text              TEXT NOT NULL,
    alias_source            TEXT NOT NULL,
    match_evidence          TEXT,
    confidence              REAL NOT NULL DEFAULT 0.0,
    merge_decision          TEXT NOT NULL DEFAULT 'pending_review',
    scope_namespace         TEXT NOT NULL,
    scope_domain            TEXT,
    scope_workspace_id      TEXT,
    scope_repo_id           TEXT,
    review_state            TEXT NOT NULL DEFAULT 'unreviewed',
    is_human_confirmed      INTEGER NOT NULL DEFAULT 0,
    is_human_confirmed_final INTEGER NOT NULL DEFAULT 0,
    superseded_by_entity_id TEXT,
    split_from_entity_id    TEXT,
    source_envelope_id      TEXT NOT NULL,
    recorded_at             TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_ea_canonical ON entity_aliases(canonical_entity_id);
CREATE INDEX IF NOT EXISTS idx_ea_alias ON entity_aliases(alias_text);
CREATE INDEX IF NOT EXISTS idx_ea_scope ON entity_aliases(scope_namespace, scope_domain);
CREATE INDEX IF NOT EXISTS idx_ea_review ON entity_aliases(review_state) WHERE review_state = 'pending_review';
CREATE INDEX IF NOT EXISTS idx_ea_envelope ON entity_aliases(source_envelope_id);

-- EVIDENCE REFS
-- Opaque by default with explicit audit-only dereference.
CREATE TABLE IF NOT EXISTS evidence_refs (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    claim_id                TEXT NOT NULL,
    claim_version_id        TEXT,
    fetch_handle            TEXT NOT NULL,
    source_authority        TEXT NOT NULL,
    source_envelope_id      TEXT NOT NULL,
    recorded_at             TEXT NOT NULL DEFAULT (datetime('now')),
    metadata                TEXT
);

CREATE INDEX IF NOT EXISTS idx_er_claim ON evidence_refs(claim_id);
CREATE INDEX IF NOT EXISTS idx_er_version ON evidence_refs(claim_version_id);
CREATE INDEX IF NOT EXISTS idx_er_envelope ON evidence_refs(source_envelope_id);

-- PROJECTION IMPORT LOG (enhanced)
-- Extends V10 import_log with scope key columns.
CREATE TABLE IF NOT EXISTS projection_import_log (
    batch_id                TEXT PRIMARY KEY,
    source_envelope_id      TEXT NOT NULL,
    schema_version          TEXT NOT NULL,
    content_digest          TEXT NOT NULL,
    source_authority        TEXT NOT NULL,
    scope_namespace         TEXT NOT NULL,
    scope_domain            TEXT,
    scope_workspace_id      TEXT,
    scope_repo_id           TEXT,
    trace_id                TEXT,
    record_count            INTEGER NOT NULL,
    claim_count             INTEGER NOT NULL DEFAULT 0,
    relation_count          INTEGER NOT NULL DEFAULT 0,
    episode_count           INTEGER NOT NULL DEFAULT 0,
    alias_count             INTEGER NOT NULL DEFAULT 0,
    evidence_count          INTEGER NOT NULL DEFAULT 0,
    status                  TEXT NOT NULL DEFAULT 'complete',
    source_exported_at      TEXT,
    transformed_at          TEXT,
    imported_at             TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_pil_envelope ON projection_import_log(source_envelope_id);
CREATE INDEX IF NOT EXISTS idx_pil_scope ON projection_import_log(scope_namespace);
CREATE INDEX IF NOT EXISTS idx_pil_imported ON projection_import_log(imported_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_pil_dedupe
    ON projection_import_log(source_envelope_id, schema_version, content_digest);

-- EPISODE LINKS
-- Projection-level episode tracking for imported episodes.
-- Stores the episode projection record as received from the bridge,
-- separate from the searchable episodes table (which includes embeddings).
CREATE TABLE IF NOT EXISTS episode_links (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    episode_id              TEXT NOT NULL,
    document_id             TEXT NOT NULL,
    cause_ids               TEXT NOT NULL DEFAULT '[]',
    effect_type             TEXT NOT NULL,
    outcome                 TEXT NOT NULL,
    confidence              REAL NOT NULL DEFAULT 0.0,
    experiment_id           TEXT,
    source_envelope_id      TEXT NOT NULL,
    source_authority        TEXT NOT NULL,
    trace_id                TEXT,
    recorded_at             TEXT NOT NULL DEFAULT (datetime('now')),
    metadata                TEXT
);

CREATE INDEX IF NOT EXISTS idx_el_episode ON episode_links(episode_id);
CREATE INDEX IF NOT EXISTS idx_el_document ON episode_links(document_id);
CREATE INDEX IF NOT EXISTS idx_el_envelope ON episode_links(source_envelope_id);

-- DERIVATION EDGES
-- Tracks lineage between projection records.
-- Each derived artifact declares an invalidation_mode.
CREATE TABLE IF NOT EXISTS derivation_edges (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    source_kind             TEXT NOT NULL,
    source_id               TEXT NOT NULL,
    target_kind             TEXT NOT NULL,
    target_id               TEXT NOT NULL,
    derivation_type         TEXT NOT NULL,
    invalidation_mode       TEXT NOT NULL DEFAULT 'on_source_change'
                            CHECK (invalidation_mode IN (
                                'on_source_change', 'on_contradiction', 'on_refutation',
                                'on_alias_split', 'on_supersession', 'on_estimator_change',
                                'on_policy_change', 'manual_only'
                            )),
    is_invalidated          INTEGER NOT NULL DEFAULT 0,
    invalidated_at          TEXT,
    invalidation_reason     TEXT,
    recorded_at             TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_de_source ON derivation_edges(source_kind, source_id);
CREATE INDEX IF NOT EXISTS idx_de_target ON derivation_edges(target_kind, target_id);
CREATE INDEX IF NOT EXISTS idx_de_invalidated ON derivation_edges(is_invalidated) WHERE is_invalidated = 1;
"#;

/// V12 migration: fix relation_versions preferred_open unique index (I011).
///
/// The V11 index keyed only on (subject_entity_id, predicate, object_anchor, scope_namespace),
/// missing scope_domain/workspace_id/repo_id/projection_family. This caused distinct scoped
/// relations to collide or be blocked incorrectly.
pub(crate) const MIGRATION_V12: &str = r#"
-- Drop the under-scoped V11 preferred_open index.
DROP INDEX IF EXISTS idx_rv_preferred;
-- Recreate with the full logical scope key per V5 spec §11.5.
CREATE UNIQUE INDEX IF NOT EXISTS idx_rv_preferred
    ON relation_versions(
        subject_entity_id, predicate, object_anchor,
        scope_namespace, COALESCE(scope_domain, ''),
        COALESCE(scope_workspace_id, ''), COALESCE(scope_repo_id, ''),
        projection_family
    )
    WHERE preferred_open = 1;
"#;

/// V13 migration: persist export_schema_version separately from import schema.
pub(crate) const MIGRATION_V13: &str = r#"
ALTER TABLE projection_import_log ADD COLUMN export_schema_version TEXT;
"#;

/// V14 migration: preserve V2 export metadata and durable failure receipts.
pub(crate) const MIGRATION_V14: &str = r#"
ALTER TABLE projection_import_log ADD COLUMN source_run_id TEXT;
ALTER TABLE projection_import_log ADD COLUMN comparability_snapshot_version TEXT;
ALTER TABLE projection_import_log ADD COLUMN direct_write INTEGER NOT NULL DEFAULT 0;
ALTER TABLE projection_import_log ADD COLUMN failure_reason TEXT;

CREATE TABLE IF NOT EXISTS projection_import_failures (
    failure_id               TEXT PRIMARY KEY,
    source_envelope_id       TEXT NOT NULL,
    schema_version           TEXT NOT NULL,
    export_schema_version    TEXT,
    content_digest           TEXT NOT NULL,
    source_authority         TEXT NOT NULL,
    scope_namespace          TEXT NOT NULL,
    scope_domain             TEXT,
    scope_workspace_id       TEXT,
    scope_repo_id            TEXT,
    trace_id                 TEXT,
    record_count             INTEGER NOT NULL,
    error_kind               TEXT NOT NULL,
    error_message            TEXT NOT NULL,
    source_exported_at       TEXT,
    transformed_at           TEXT,
    failed_at                TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_pif_envelope ON projection_import_failures(source_envelope_id);
CREATE INDEX IF NOT EXISTS idx_pif_scope ON projection_import_failures(scope_namespace);
CREATE INDEX IF NOT EXISTS idx_pif_failed_at ON projection_import_failures(failed_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_pif_dedupe
    ON projection_import_failures(source_envelope_id, schema_version, content_digest);
"#;

/// V15 migration: preserve canonical evidence-bundle receipts on import log rows.
pub(crate) const MIGRATION_V15: &str = r#"
ALTER TABLE projection_import_log ADD COLUMN evidence_bundle_id TEXT;
ALTER TABLE projection_import_log ADD COLUMN evidence_bundle_json TEXT;

ALTER TABLE projection_import_failures ADD COLUMN source_run_id TEXT;
ALTER TABLE projection_import_failures ADD COLUMN comparability_snapshot_version TEXT;
ALTER TABLE projection_import_failures ADD COLUMN direct_write INTEGER NOT NULL DEFAULT 0;
ALTER TABLE projection_import_failures ADD COLUMN evidence_bundle_id TEXT;
ALTER TABLE projection_import_failures ADD COLUMN evidence_bundle_json TEXT;
"#;

/// V16 migration: preserve rebuildable kernel payloads on import receipts.
pub(crate) const MIGRATION_V16: &str = r#"
ALTER TABLE projection_import_log ADD COLUMN kernel_payload_json TEXT;
ALTER TABLE projection_import_failures ADD COLUMN kernel_payload_json TEXT;
"#;

/// V17 migration: preserve v9 episode-bundle and execution-context proof surfaces.
pub(crate) const MIGRATION_V17: &str = r#"
ALTER TABLE projection_import_log ADD COLUMN episode_bundle_id TEXT;
ALTER TABLE projection_import_log ADD COLUMN episode_bundle_json TEXT;
ALTER TABLE projection_import_log ADD COLUMN execution_context_json TEXT;
ALTER TABLE projection_import_failures ADD COLUMN episode_bundle_id TEXT;
ALTER TABLE projection_import_failures ADD COLUMN episode_bundle_json TEXT;
ALTER TABLE projection_import_failures ADD COLUMN execution_context_json TEXT;
"#;

/// Check if a projection import batch has already been ingested.
pub(crate) fn check_projection_import_exists(
    conn: &rusqlite::Connection,
    source_envelope_id: &str,
    schema_version: &str,
    content_digest: &str,
) -> Result<bool, MemoryError> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM projection_import_log
             WHERE source_envelope_id = ?1 AND schema_version = ?2 AND content_digest = ?3
               AND status = 'complete'",
            rusqlite::params![source_envelope_id, schema_version, content_digest],
            |row| row.get(0),
        )
        .unwrap_or(0);
    Ok(count > 0)
}

pub(crate) fn claim_version_source_envelope(
    conn: &rusqlite::Connection,
    claim_version_id: &str,
) -> Result<Option<String>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT source_envelope_id FROM claim_versions WHERE claim_version_id = ?1 LIMIT 1",
    )?;
    let mut rows = stmt.query(rusqlite::params![claim_version_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

pub(crate) fn relation_version_source_envelope(
    conn: &rusqlite::Connection,
    relation_version_id: &str,
) -> Result<Option<String>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT source_envelope_id FROM relation_versions WHERE relation_version_id = ?1 LIMIT 1",
    )?;
    let mut rows = stmt.query(rusqlite::params![relation_version_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

/// Insert a claim version record.
pub(crate) fn insert_claim_version(
    tx: &rusqlite::Transaction<'_>,
    cv: &ClaimVersionRow,
) -> Result<(), MemoryError> {
    tx.execute(
        "INSERT INTO claim_versions (
            claim_version_id, claim_id, claim_state, projection_family,
            subject_entity_id, predicate, object_anchor,
            scope_namespace, scope_domain, scope_workspace_id, scope_repo_id,
            valid_from, valid_to, recorded_at, preferred_open,
            source_envelope_id, source_authority, trace_id,
            freshness, contradiction_status, supersedes_claim_version_id,
            content, confidence, content_digest, metadata
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
            ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21,
            ?22, ?23, ?24, ?25
        )",
        rusqlite::params![
            cv.claim_version_id,
            cv.claim_id,
            cv.claim_state,
            cv.projection_family,
            cv.subject_entity_id,
            cv.predicate,
            cv.object_anchor,
            cv.scope_namespace,
            cv.scope_domain,
            cv.scope_workspace_id,
            cv.scope_repo_id,
            cv.valid_from,
            cv.valid_to,
            cv.recorded_at,
            cv.preferred_open as i32,
            cv.source_envelope_id,
            cv.source_authority,
            cv.trace_id,
            cv.freshness,
            cv.contradiction_status,
            cv.supersedes_claim_version_id,
            cv.content,
            cv.confidence,
            cv.content_digest,
            cv.metadata,
        ],
    )?;
    Ok(())
}

/// Insert a relation version record.
pub(crate) fn insert_relation_version(
    tx: &rusqlite::Transaction<'_>,
    rv: &RelationVersionRow,
) -> Result<(), MemoryError> {
    tx.execute(
        "INSERT INTO relation_versions (
            relation_version_id, subject_entity_id, predicate, object_anchor,
            scope_namespace, scope_domain, scope_workspace_id, scope_repo_id,
            claim_id, source_episode_id, valid_from, valid_to,
            recorded_at, preferred_open, supersedes_relation_version_id,
            contradiction_status, source_confidence, projection_family,
            source_envelope_id, source_authority, trace_id, freshness, metadata
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
            ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23
        )",
        rusqlite::params![
            rv.relation_version_id,
            rv.subject_entity_id,
            rv.predicate,
            rv.object_anchor,
            rv.scope_namespace,
            rv.scope_domain,
            rv.scope_workspace_id,
            rv.scope_repo_id,
            rv.claim_id,
            rv.source_episode_id,
            rv.valid_from,
            rv.valid_to,
            rv.recorded_at,
            rv.preferred_open as i32,
            rv.supersedes_relation_version_id,
            rv.contradiction_status,
            rv.source_confidence,
            rv.projection_family,
            rv.source_envelope_id,
            rv.source_authority,
            rv.trace_id,
            rv.freshness,
            rv.metadata,
        ],
    )?;
    Ok(())
}

/// Insert an entity alias record.
pub(crate) fn insert_entity_alias(
    tx: &rusqlite::Transaction<'_>,
    ea: &EntityAliasRow,
) -> Result<(), MemoryError> {
    tx.execute(
        "INSERT INTO entity_aliases (
            canonical_entity_id, alias_text, alias_source, match_evidence,
            confidence, merge_decision,
            scope_namespace, scope_domain, scope_workspace_id, scope_repo_id,
            review_state, is_human_confirmed, is_human_confirmed_final,
            superseded_by_entity_id, split_from_entity_id,
            source_envelope_id, recorded_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17
        )",
        rusqlite::params![
            ea.canonical_entity_id,
            ea.alias_text,
            ea.alias_source,
            ea.match_evidence,
            ea.confidence,
            ea.merge_decision,
            ea.scope_namespace,
            ea.scope_domain,
            ea.scope_workspace_id,
            ea.scope_repo_id,
            ea.review_state,
            ea.is_human_confirmed as i32,
            ea.is_human_confirmed_final as i32,
            ea.superseded_by_entity_id,
            ea.split_from_entity_id,
            ea.source_envelope_id,
            ea.recorded_at,
        ],
    )?;
    Ok(())
}

/// Insert an evidence ref record.
pub(crate) fn insert_evidence_ref(
    tx: &rusqlite::Transaction<'_>,
    er: &EvidenceRefRow,
) -> Result<(), MemoryError> {
    tx.execute(
        "INSERT INTO evidence_refs (
            claim_id, claim_version_id, fetch_handle,
            source_authority, source_envelope_id, recorded_at, metadata
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            er.claim_id,
            er.claim_version_id,
            er.fetch_handle,
            er.source_authority,
            er.source_envelope_id,
            er.recorded_at,
            er.metadata,
        ],
    )?;
    Ok(())
}

/// Insert a projection import log entry.
pub(crate) fn insert_projection_import_log(
    tx: &rusqlite::Transaction<'_>,
    log: &ProjectionImportLogRow,
) -> Result<(), MemoryError> {
    tx.execute(
        "INSERT INTO projection_import_log (
            batch_id, source_envelope_id, schema_version, export_schema_version,
            content_digest, source_authority, scope_namespace, scope_domain,
            scope_workspace_id, scope_repo_id, trace_id, record_count,
            claim_count, relation_count, episode_count, alias_count,
            evidence_count, status, source_exported_at, transformed_at, imported_at,
            source_run_id, comparability_snapshot_version, direct_write, failure_reason,
            evidence_bundle_id, evidence_bundle_json, episode_bundle_id, episode_bundle_json,
            execution_context_json, kernel_payload_json
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
            ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21,
            ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31
        )
        ON CONFLICT(source_envelope_id, schema_version, content_digest) DO UPDATE SET
            batch_id = excluded.batch_id,
            export_schema_version = excluded.export_schema_version,
            source_authority = excluded.source_authority,
            scope_namespace = excluded.scope_namespace,
            scope_domain = excluded.scope_domain,
            scope_workspace_id = excluded.scope_workspace_id,
            scope_repo_id = excluded.scope_repo_id,
            trace_id = excluded.trace_id,
            record_count = excluded.record_count,
            claim_count = excluded.claim_count,
            relation_count = excluded.relation_count,
            episode_count = excluded.episode_count,
            alias_count = excluded.alias_count,
            evidence_count = excluded.evidence_count,
            status = excluded.status,
            source_exported_at = excluded.source_exported_at,
            transformed_at = excluded.transformed_at,
            imported_at = excluded.imported_at,
            source_run_id = excluded.source_run_id,
            comparability_snapshot_version = excluded.comparability_snapshot_version,
            direct_write = excluded.direct_write,
            failure_reason = excluded.failure_reason,
            evidence_bundle_id = excluded.evidence_bundle_id,
            evidence_bundle_json = excluded.evidence_bundle_json,
            episode_bundle_id = excluded.episode_bundle_id,
            episode_bundle_json = excluded.episode_bundle_json,
            execution_context_json = excluded.execution_context_json,
            kernel_payload_json = excluded.kernel_payload_json",
        rusqlite::params![
            log.batch_id,
            log.source_envelope_id,
            log.schema_version,
            log.export_schema_version,
            log.content_digest,
            log.source_authority,
            log.scope_namespace,
            log.scope_domain,
            log.scope_workspace_id,
            log.scope_repo_id,
            log.trace_id,
            log.record_count as i64,
            log.claim_count as i64,
            log.relation_count as i64,
            log.episode_count as i64,
            log.alias_count as i64,
            log.evidence_count as i64,
            log.status,
            log.source_exported_at,
            log.transformed_at,
            log.imported_at,
            log.source_run_id,
            log.comparability_snapshot_version,
            log.direct_write as i32,
            log.failure_reason,
            log.evidence_bundle_id,
            log.evidence_bundle_json,
            log.episode_bundle_id,
            log.episode_bundle_json,
            log.execution_context_json,
            log.kernel_payload_json,
        ],
    )?;
    Ok(())
}

/// Upsert a projection import log entry outside the main import transaction.
pub(crate) fn upsert_projection_import_log_conn(
    conn: &rusqlite::Connection,
    log: &ProjectionImportLogRow,
) -> Result<(), MemoryError> {
    conn.execute(
        "INSERT INTO projection_import_log (
            batch_id, source_envelope_id, schema_version, export_schema_version,
            content_digest, source_authority, scope_namespace, scope_domain,
            scope_workspace_id, scope_repo_id, trace_id, record_count,
            claim_count, relation_count, episode_count, alias_count,
            evidence_count, status, source_exported_at, transformed_at, imported_at,
            source_run_id, comparability_snapshot_version, direct_write, failure_reason,
            evidence_bundle_id, evidence_bundle_json, episode_bundle_id, episode_bundle_json,
            execution_context_json, kernel_payload_json
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
            ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21,
            ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31
        )
        ON CONFLICT(source_envelope_id, schema_version, content_digest) DO UPDATE SET
            batch_id = excluded.batch_id,
            export_schema_version = excluded.export_schema_version,
            source_authority = excluded.source_authority,
            scope_namespace = excluded.scope_namespace,
            scope_domain = excluded.scope_domain,
            scope_workspace_id = excluded.scope_workspace_id,
            scope_repo_id = excluded.scope_repo_id,
            trace_id = excluded.trace_id,
            record_count = excluded.record_count,
            claim_count = excluded.claim_count,
            relation_count = excluded.relation_count,
            episode_count = excluded.episode_count,
            alias_count = excluded.alias_count,
            evidence_count = excluded.evidence_count,
            status = excluded.status,
            source_exported_at = excluded.source_exported_at,
            transformed_at = excluded.transformed_at,
            imported_at = excluded.imported_at,
            source_run_id = excluded.source_run_id,
            comparability_snapshot_version = excluded.comparability_snapshot_version,
            direct_write = excluded.direct_write,
            failure_reason = excluded.failure_reason,
            evidence_bundle_id = excluded.evidence_bundle_id,
            evidence_bundle_json = excluded.evidence_bundle_json,
            episode_bundle_id = excluded.episode_bundle_id,
            episode_bundle_json = excluded.episode_bundle_json,
            execution_context_json = excluded.execution_context_json,
            kernel_payload_json = excluded.kernel_payload_json",
        rusqlite::params![
            log.batch_id,
            log.source_envelope_id,
            log.schema_version,
            log.export_schema_version,
            log.content_digest,
            log.source_authority,
            log.scope_namespace,
            log.scope_domain,
            log.scope_workspace_id,
            log.scope_repo_id,
            log.trace_id,
            log.record_count as i64,
            log.claim_count as i64,
            log.relation_count as i64,
            log.episode_count as i64,
            log.alias_count as i64,
            log.evidence_count as i64,
            log.status,
            log.source_exported_at,
            log.transformed_at,
            log.imported_at,
            log.source_run_id,
            log.comparability_snapshot_version,
            log.direct_write as i32,
            log.failure_reason,
            log.evidence_bundle_id,
            log.evidence_bundle_json,
            log.episode_bundle_id,
            log.episode_bundle_json,
            log.execution_context_json,
            log.kernel_payload_json,
        ],
    )?;
    Ok(())
}

/// Insert or update a durable failed import receipt.
pub(crate) fn insert_projection_import_failure(
    conn: &rusqlite::Connection,
    row: &ProjectionImportFailureRow,
) -> Result<(), MemoryError> {
    conn.execute(
        "INSERT OR REPLACE INTO projection_import_failures (
            failure_id, source_envelope_id, schema_version, export_schema_version,
            content_digest, source_authority, scope_namespace, scope_domain,
            scope_workspace_id, scope_repo_id, trace_id, record_count,
            error_kind, error_message, source_exported_at, transformed_at, failed_at,
            source_run_id, comparability_snapshot_version, direct_write,
            evidence_bundle_id, evidence_bundle_json, episode_bundle_id, episode_bundle_json,
            execution_context_json, kernel_payload_json
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
            ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26
        )",
        rusqlite::params![
            row.failure_id,
            row.source_envelope_id,
            row.schema_version,
            row.export_schema_version,
            row.content_digest,
            row.source_authority,
            row.scope_namespace,
            row.scope_domain,
            row.scope_workspace_id,
            row.scope_repo_id,
            row.trace_id,
            row.record_count as i64,
            row.error_kind,
            row.error_message,
            row.source_exported_at,
            row.transformed_at,
            row.failed_at,
            row.source_run_id,
            row.comparability_snapshot_version,
            row.direct_write as i32,
            row.evidence_bundle_id,
            row.evidence_bundle_json,
            row.episode_bundle_id,
            row.episode_bundle_json,
            row.execution_context_json,
            row.kernel_payload_json,
        ],
    )?;
    Ok(())
}

/// Insert an episode link record.
pub(crate) fn insert_episode_link(
    tx: &rusqlite::Transaction<'_>,
    el: &EpisodeLinkRow,
) -> Result<(), MemoryError> {
    tx.execute(
        "INSERT INTO episode_links (
            episode_id, document_id, cause_ids, effect_type, outcome,
            confidence, experiment_id, source_envelope_id, source_authority,
            trace_id, recorded_at, metadata
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            el.episode_id,
            el.document_id,
            el.cause_ids,
            el.effect_type,
            el.outcome,
            el.confidence,
            el.experiment_id,
            el.source_envelope_id,
            el.source_authority,
            el.trace_id,
            el.recorded_at,
            el.metadata,
        ],
    )?;
    Ok(())
}

/// Insert a derivation edge with invalidation mode.
pub(crate) fn insert_derivation_edge(
    tx: &rusqlite::Transaction<'_>,
    source_kind: &str,
    source_id: &str,
    target_kind: &str,
    target_id: &str,
    derivation_type: &str,
    invalidation_mode: &str,
) -> Result<(), MemoryError> {
    tx.execute(
        "INSERT INTO derivation_edges (
            source_kind, source_id, target_kind, target_id,
            derivation_type, invalidation_mode
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            source_kind,
            source_id,
            target_kind,
            target_id,
            derivation_type,
            invalidation_mode
        ],
    )?;
    Ok(())
}

/// A row from the derivation_edges table.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct DerivationEdgeRow {
    pub id: i64,
    pub source_kind: String,
    pub source_id: String,
    pub target_kind: String,
    pub target_id: String,
    pub derivation_type: String,
    pub invalidation_mode: String,
    pub is_invalidated: bool,
    pub invalidated_at: Option<String>,
    pub invalidation_reason: Option<String>,
    pub recorded_at: String,
}

/// Query derivation edges by source.
#[allow(dead_code)]
pub(crate) fn query_derivation_edges_by_source(
    conn: &rusqlite::Connection,
    source_kind: &str,
    source_id: &str,
) -> Result<Vec<DerivationEdgeRow>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, source_kind, source_id, target_kind, target_id,
                derivation_type, invalidation_mode, is_invalidated,
                invalidated_at, invalidation_reason, recorded_at
         FROM derivation_edges
         WHERE source_kind = ?1 AND source_id = ?2
         ORDER BY recorded_at ASC",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![source_kind, source_id], |row| {
            Ok(DerivationEdgeRow {
                id: row.get(0)?,
                source_kind: row.get(1)?,
                source_id: row.get(2)?,
                target_kind: row.get(3)?,
                target_id: row.get(4)?,
                derivation_type: row.get(5)?,
                invalidation_mode: row.get(6)?,
                is_invalidated: row.get(7)?,
                invalidated_at: row.get(8)?,
                invalidation_reason: row.get(9)?,
                recorded_at: row.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Invalidate derivation edges matching a trigger mode, bounded by source.
///
/// Returns the number of edges invalidated.
pub(crate) fn invalidate_derivation_edges(
    conn: &rusqlite::Connection,
    source_kind: &str,
    source_id: &str,
    trigger_mode: &str,
    reason: &str,
) -> Result<usize, MemoryError> {
    let now = chrono::Utc::now().to_rfc3339();
    let count = conn.execute(
        "UPDATE derivation_edges
         SET is_invalidated = 1, invalidated_at = ?1, invalidation_reason = ?2
         WHERE source_kind = ?3 AND source_id = ?4
           AND invalidation_mode = ?5
           AND is_invalidated = 0",
        rusqlite::params![now, reason, source_kind, source_id, trigger_mode],
    )?;
    Ok(count)
}

/// Get all invalidated derivation edge targets for bounded recomputation.
#[allow(dead_code)]
pub(crate) fn list_invalidated_targets(
    conn: &rusqlite::Connection,
    limit: usize,
) -> Result<Vec<DerivationEdgeRow>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, source_kind, source_id, target_kind, target_id,
                derivation_type, invalidation_mode, is_invalidated,
                invalidated_at, invalidation_reason, recorded_at
         FROM derivation_edges
         WHERE is_invalidated = 1
         ORDER BY invalidated_at ASC
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![limit as i64], |row| {
            Ok(DerivationEdgeRow {
                id: row.get(0)?,
                source_kind: row.get(1)?,
                source_id: row.get(2)?,
                target_kind: row.get(3)?,
                target_id: row.get(4)?,
                derivation_type: row.get(5)?,
                invalidation_mode: row.get(6)?,
                is_invalidated: row.get(7)?,
                invalidated_at: row.get(8)?,
                invalidation_reason: row.get(9)?,
                recorded_at: row.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// List preferred claim versions for the same logical key with interval metadata.
///
/// This is used to enforce temporal integrity invariants for preferred versions
/// in the canonical projection import path.
#[allow(clippy::type_complexity)]
pub(crate) fn query_preferred_claim_intervals(
    conn: &rusqlite::Connection,
    claim_id: &str,
) -> Result<Vec<(String, Option<String>, Option<String>)>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT claim_version_id, valid_from, valid_to
           FROM claim_versions
          WHERE claim_id = ?1
            AND preferred_open = 1",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![claim_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// List preferred relation versions for the same logical key with interval metadata.
///
/// Logical key is `(subject_entity_id, predicate, object_anchor, scope_key, projection_family)`.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn query_preferred_relation_intervals(
    conn: &rusqlite::Connection,
    subject_entity_id: &str,
    predicate: &str,
    object_anchor: &str,
    scope_namespace: &str,
    scope_domain: Option<&str>,
    scope_workspace_id: Option<&str>,
    scope_repo_id: Option<&str>,
    projection_family: &str,
) -> Result<Vec<(String, Option<String>, Option<String>)>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT relation_version_id, valid_from, valid_to
           FROM relation_versions
          WHERE subject_entity_id = ?1
            AND predicate = ?2
            AND object_anchor = ?3
            AND scope_namespace = ?4
            AND (?5 IS NULL AND scope_domain IS NULL OR scope_domain = ?5)
            AND (?6 IS NULL AND scope_workspace_id IS NULL OR scope_workspace_id = ?6)
            AND (?7 IS NULL AND scope_repo_id IS NULL OR scope_repo_id = ?7)
            AND projection_family = ?8
            AND preferred_open = 1",
    )?;
    let rows = stmt
        .query_map(
            rusqlite::params![
                subject_entity_id,
                predicate,
                object_anchor,
                scope_namespace,
                scope_domain,
                scope_workspace_id,
                scope_repo_id,
                projection_family,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}
