//! V10 legacy projection import boundary.
//!
//! ## Phase status: compatibility / migration-only
//!
//! This module provides the **legacy** (V10) import path. It remains functional
//! for backward compatibility during the migration cycle but is **not the
//! canonical import path** for new integrations.
//!
//! **Canonical path**: Use `MemoryStore::import_projection_batch()` (V11+),
//! which accepts `ProjectionImportBatchV3` output from `forge-memory-bridge`.
//! `ProjectionImportBatchV2` remains a compatibility-normalized import shape.
//!
//! **Removal condition**: This module and its types (`ImportEnvelope`,
//! `ImportRecord`, `import_envelope()`) will be removed once all callers have
//! migrated to the bridge pipeline. The V10 `import_log` table is retained
//! read-only for audit after removal.
//!
//! ## Authority boundary
//!
//! `semantic-memory` owns queryable knowledge persistence. This module accepts
//! **already-interpreted** projection inputs and writes them atomically. It does
//! NOT interpret raw verification data, decide promotion policy, or contain
//! Forge-specific transformation logic.
//!
//! ## Import semantics
//!
//! - **Atomic per envelope**: all records in an envelope are committed together
//!   or not at all.
//! - **Idempotent**: re-importing the same envelope (identified by
//!   `envelope_id` + `schema_version` + `content_digest`) is a safe no-op.
//! - **No partial visibility**: if any record fails, the entire envelope is
//!   rolled back.
//! - **Provenance preserved**: every imported record carries envelope metadata
//!   in its JSON metadata for traceability.

#![allow(deprecated)]

use crate::error::MemoryError;
use crate::types::TraceId;
use serde::{Deserialize, Serialize};
pub use stack_ids::EnvelopeId;

// ─── Envelope Types ────────────────────────────────────────────

/// An import envelope containing projection records to be atomically ingested.
///
/// ## Phase status: compatibility / migration-only
///
/// This type is the V10 legacy import format. New integrations should use
/// `MemoryStore::import_projection_batch()` with `ProjectionImportBatchV3`
/// from `forge-memory-bridge`.
///
/// **Removal condition**: removed when all callers migrate to the bridge pipeline.
///
/// The envelope is the unit of atomicity: all records are committed together
/// or the entire import is rolled back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportEnvelope {
    /// Unique envelope identity (assigned by source authority).
    pub envelope_id: EnvelopeId,
    /// Schema version of the export format.
    pub schema_version: String,
    /// Content digest (e.g. blake3 hash) for deduplication.
    pub content_digest: String,
    /// Which system produced this envelope (e.g. "forge", "external").
    pub source_authority: String,
    /// Cross-crate trace identifier for auditability.
    pub trace_id: Option<TraceId>,
    /// Target namespace for all records in this envelope.
    pub namespace: String,
    /// The projection records to import.
    pub records: Vec<ImportRecord>,
}

impl ImportEnvelope {
    /// Validate envelope structure. Returns an error if the envelope is malformed.
    pub fn validate(&self) -> Result<(), MemoryError> {
        if self.envelope_id.is_empty() {
            return Err(MemoryError::ImportInvalid {
                reason: "envelope_id must not be empty".into(),
            });
        }
        if self.schema_version.is_empty() {
            return Err(MemoryError::ImportInvalid {
                reason: "schema_version must not be empty".into(),
            });
        }
        if self.content_digest.is_empty() {
            return Err(MemoryError::ImportInvalid {
                reason: "content_digest must not be empty".into(),
            });
        }
        if self.source_authority.is_empty() {
            return Err(MemoryError::ImportInvalid {
                reason: "source_authority must not be empty".into(),
            });
        }
        if self.namespace.is_empty() {
            return Err(MemoryError::ImportInvalid {
                reason: "namespace must not be empty".into(),
            });
        }
        if self.records.is_empty() {
            return Err(MemoryError::ImportInvalid {
                reason: "envelope must contain at least one record".into(),
            });
        }
        for (i, record) in self.records.iter().enumerate() {
            record.validate().map_err(|e| MemoryError::ImportInvalid {
                reason: format!("record[{i}]: {e}"),
            })?;
        }
        Ok(())
    }

    /// Dedupe key: (envelope_id, schema_version, content_digest).
    pub(crate) fn dedupe_key(&self) -> (&str, &str, &str) {
        (
            self.envelope_id.as_str(),
            &self.schema_version,
            &self.content_digest,
        )
    }
}

/// A single record within an import envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImportRecord {
    /// A fact (claim/knowledge projection).
    Fact {
        /// The fact content text.
        content: String,
        /// Source attribution.
        source: Option<String>,
        /// Additional metadata.
        metadata: Option<serde_json::Value>,
    },
    /// An episode (causal record) attached to an existing document.
    Episode {
        /// The document this episode is attached to.
        document_id: String,
        /// Episode metadata.
        meta: crate::types::EpisodeMeta,
    },
}

impl ImportRecord {
    fn validate(&self) -> Result<(), MemoryError> {
        match self {
            ImportRecord::Fact { content, .. } => {
                if content.is_empty() {
                    return Err(MemoryError::ImportInvalid {
                        reason: "fact content must not be empty".into(),
                    });
                }
            }
            ImportRecord::Episode { document_id, .. } => {
                if document_id.is_empty() {
                    return Err(MemoryError::ImportInvalid {
                        reason: "episode document_id must not be empty".into(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Text content for embedding generation.
    pub fn content_text(&self) -> &str {
        match self {
            ImportRecord::Fact { content, .. } => content,
            ImportRecord::Episode { meta, .. } => &meta.effect_type,
        }
    }
}

// ─── Import Status / Receipt ───────────────────────────────────

/// Lifecycle status of an import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportStatus {
    /// Successfully imported all records.
    Complete,
    /// Envelope was already imported (idempotent no-op).
    AlreadyImported,
    /// Import was attempted but aborted.
    Aborted { reason: String },
}

impl ImportStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Complete => "complete",
            Self::AlreadyImported => "already_imported",
            Self::Aborted { .. } => "aborted",
        }
    }

    pub fn from_str_value(s: &str) -> Self {
        match s {
            "complete" => Self::Complete,
            "already_imported" => Self::AlreadyImported,
            s if s.starts_with("aborted:") => Self::Aborted {
                reason: s.strip_prefix("aborted:").unwrap_or("").to_string(),
            },
            _ => Self::Aborted {
                reason: format!("unknown status: {s}"),
            },
        }
    }

    /// Encode for storage.
    pub(crate) fn to_db_string(&self) -> String {
        match self {
            Self::Complete => "complete".into(),
            Self::AlreadyImported => "already_imported".into(),
            Self::Aborted { reason } => format!("aborted:{reason}"),
        }
    }
}

/// Receipt confirming the outcome of an import attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportReceipt {
    /// Envelope that was imported.
    pub envelope_id: EnvelopeId,
    /// Schema version.
    pub schema_version: String,
    /// Content digest.
    pub content_digest: String,
    /// Import outcome.
    pub status: ImportStatus,
    /// Number of records in the envelope.
    pub record_count: usize,
    /// ISO 8601 timestamp of the import.
    pub imported_at: String,
    /// Whether this was a duplicate (idempotent no-op).
    pub was_duplicate: bool,
    /// Trace ID if provided.
    pub trace_id: Option<TraceId>,
}

// ─── Projection Freshness ──────────────────────────────────────

/// Freshness/lifecycle status for imported projections.
///
/// Used by the runtime to explain projection state to callers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportProjectionFreshness {
    /// Projection was recently imported and is current.
    Current,
    /// Projection exists but the last import is older than the staleness threshold.
    Stale { last_import_at: String },
    /// A newer envelope has superseded this projection's data.
    Superseded { superseded_by: EnvelopeId },
    /// The last import attempt for this scope/source failed.
    ImportFailed { error: String, attempted_at: String },
    /// No import has ever been recorded for this scope.
    NeverImported,
}

// ─── DB Operations ─────────────────────────────────────────────

/// V10 migration SQL: import log table.
pub(crate) const MIGRATION_V10: &str = r#"
CREATE TABLE IF NOT EXISTS import_log (
    envelope_id     TEXT NOT NULL,
    schema_version  TEXT NOT NULL,
    content_digest  TEXT NOT NULL,
    source_authority TEXT NOT NULL,
    namespace       TEXT NOT NULL,
    trace_id        TEXT,
    record_count    INTEGER NOT NULL,
    status          TEXT NOT NULL DEFAULT 'complete',
    imported_at     TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (envelope_id, schema_version, content_digest)
);

CREATE INDEX IF NOT EXISTS idx_import_log_namespace ON import_log(namespace);
CREATE INDEX IF NOT EXISTS idx_import_log_imported_at ON import_log(imported_at DESC);
CREATE INDEX IF NOT EXISTS idx_import_log_source ON import_log(source_authority);
"#;

/// Check if an envelope has already been imported.
pub(crate) fn check_import_exists(
    conn: &rusqlite::Connection,
    envelope_id: &str,
    schema_version: &str,
    content_digest: &str,
) -> Result<bool, MemoryError> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM import_log
             WHERE envelope_id = ?1 AND schema_version = ?2 AND content_digest = ?3",
            rusqlite::params![envelope_id, schema_version, content_digest],
            |row| row.get(0),
        )
        .unwrap_or(0);
    Ok(count > 0)
}

/// Insert an import log entry within a transaction.
pub(crate) fn insert_import_log(
    tx: &rusqlite::Transaction<'_>,
    envelope: &ImportEnvelope,
    status: &ImportStatus,
    record_count: usize,
) -> Result<(), MemoryError> {
    tx.execute(
        "INSERT OR REPLACE INTO import_log
         (envelope_id, schema_version, content_digest, source_authority,
          namespace, trace_id, record_count, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            envelope.envelope_id.as_str(),
            envelope.schema_version,
            envelope.content_digest,
            envelope.source_authority,
            envelope.namespace,
            envelope.trace_id.as_ref().map(|t| t.as_str()),
            record_count as i64,
            status.to_db_string(),
        ],
    )?;
    Ok(())
}

/// Query import receipts, optionally filtered by namespace.
pub(crate) fn query_import_log(
    conn: &rusqlite::Connection,
    namespace: Option<&str>,
    limit: usize,
) -> Result<Vec<ImportReceipt>, MemoryError> {
    let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(ns) = namespace {
        (
            "SELECT envelope_id, schema_version, content_digest, status,
                    record_count, imported_at, trace_id
             FROM import_log
             WHERE namespace = ?1
             ORDER BY imported_at DESC
             LIMIT ?2",
            vec![
                Box::new(ns.to_string()) as Box<dyn rusqlite::types::ToSql>,
                Box::new(limit as i64),
            ],
        )
    } else {
        (
            "SELECT envelope_id, schema_version, content_digest, status,
                    record_count, imported_at, trace_id
             FROM import_log
             ORDER BY imported_at DESC
             LIMIT ?1",
            vec![Box::new(limit as i64) as Box<dyn rusqlite::types::ToSql>],
        )
    };

    let mut stmt = conn.prepare(sql)?;
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let rows = stmt
        .query_map(params_refs.as_slice(), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows
        .into_iter()
        .map(
            |(
                envelope_id,
                schema_version,
                content_digest,
                status,
                record_count,
                imported_at,
                trace_id,
            )| {
                let status_parsed = ImportStatus::from_str_value(&status);
                let was_duplicate = matches!(status_parsed, ImportStatus::AlreadyImported);
                ImportReceipt {
                    envelope_id: EnvelopeId(envelope_id),
                    schema_version,
                    content_digest,
                    status: status_parsed,
                    record_count: record_count as usize,
                    imported_at,
                    was_duplicate,
                    trace_id: trace_id.map(TraceId::new),
                }
            },
        )
        .collect())
}

/// Get the most recent import timestamp for a namespace.
pub(crate) fn last_import_at(
    conn: &rusqlite::Connection,
    namespace: &str,
) -> Result<Option<String>, MemoryError> {
    // Check both V10 (import_log) and V11 (projection_import_log) tables,
    // returning the most recent timestamp from either.
    // INTENTIONAL: V10 import_log table may not exist on newer schemas; None is expected
    let v10: Option<String> = conn
        .query_row(
            "SELECT imported_at FROM import_log
             WHERE namespace = ?1 AND status = 'complete'
             ORDER BY imported_at DESC LIMIT 1",
            rusqlite::params![namespace],
            |row| row.get(0),
        )
        .ok();

    // INTENTIONAL: V11 projection_import_log table may not exist on older schemas; None is expected
    let v11: Option<String> = conn
        .query_row(
            "SELECT imported_at FROM projection_import_log
             WHERE scope_namespace = ?1 AND status = 'complete'
             ORDER BY imported_at DESC LIMIT 1",
            rusqlite::params![namespace],
            |row| row.get(0),
        )
        .ok();

    // Return the more recent of the two
    let result = match (v10, v11) {
        (Some(a), Some(b)) => Some(if a >= b { a } else { b }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    Ok(result)
}
