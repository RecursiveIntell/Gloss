use crate::{
    db, json_compat_import,
    projection_batch::{encode_merge_decision, encode_review_state, ProjectionImportBatchLike},
    projection_storage, MemoryError, MemoryStore,
};
use forge_memory_bridge::{
    ImportProjectionRecord, ProjectionImportBatchV2, ProjectionImportBatchV3,
    PROJECTION_IMPORT_BATCH_V2_SCHEMA,
};
use stack_ids::{DigestBuilder, ScopeKey};

pub(crate) fn projection_import_failure_id(
    source_envelope_id: &str,
    schema_version: &str,
    content_digest: &str,
) -> String {
    let mut builder = DigestBuilder::new();
    builder
        .update_str(source_envelope_id)
        .separator()
        .update_str(schema_version)
        .separator()
        .update_str(content_digest);
    format!("projection-import-failure:{}", builder.finalize().hex())
}

fn serialized_import_evidence_bundle(
    batch: &ProjectionImportBatchV2,
) -> Result<(Option<String>, Option<String>), MemoryError> {
    let Some(bundle) = batch.evidence_bundle.as_ref() else {
        return Ok((None, None));
    };

    let bundle_json = serde_json::to_string(bundle).map_err(|err| {
        MemoryError::Other(format!(
            "failed to serialize canonical evidence bundle {} for import receipt: {}",
            bundle.id, err
        ))
    })?;

    Ok((Some(bundle.id.as_str().to_string()), Some(bundle_json)))
}

fn serialized_episode_bundle(
    batch: &ProjectionImportBatchV2,
) -> Result<(Option<String>, Option<String>), MemoryError> {
    let Some(bundle) = batch.episode_bundle.as_ref() else {
        return Ok((None, None));
    };

    let bundle_json = serde_json::to_string(bundle).map_err(|err| {
        MemoryError::Other(format!(
            "failed to serialize episode bundle {} for import receipt: {}",
            bundle.bundle_id, err
        ))
    })?;

    Ok((Some(bundle.bundle_id.clone()), Some(bundle_json)))
}

fn serialized_execution_context(
    batch: &ProjectionImportBatchV2,
) -> Result<Option<String>, MemoryError> {
    batch
        .execution_context
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|err| {
            MemoryError::Other(format!(
                "failed to serialize execution context for import receipt: {}",
                err
            ))
        })
}

#[allow(clippy::too_many_arguments)]
fn build_projection_import_log_row(
    batch: &ProjectionImportBatchV2,
    batch_id: String,
    status: &str,
    imported_at: String,
    failure_reason: Option<String>,
    kernel_payload_json: Option<String>,
    record_count: usize,
    claim_count: usize,
    relation_count: usize,
    episode_count: usize,
    alias_count: usize,
    evidence_count: usize,
) -> Result<projection_storage::ProjectionImportLogRow, MemoryError> {
    let (evidence_bundle_id, evidence_bundle_json) = serialized_import_evidence_bundle(batch)?;
    let (episode_bundle_id, episode_bundle_json) = serialized_episode_bundle(batch)?;
    let execution_context_json = serialized_execution_context(batch)?;

    Ok(projection_storage::ProjectionImportLogRow {
        batch_id,
        source_envelope_id: batch.source_envelope_id.as_str().to_string(),
        schema_version: batch.schema_version.clone(),
        export_schema_version: batch.export_schema_version.clone(),
        content_digest: batch.content_digest.hex().to_string(),
        source_authority: batch.source_authority.clone(),
        scope_namespace: batch.scope_key.namespace.clone(),
        scope_domain: batch.scope_key.domain.clone(),
        scope_workspace_id: batch.scope_key.workspace_id.clone(),
        scope_repo_id: batch.scope_key.repo_id.clone(),
        trace_id: batch.trace_ctx.as_ref().map(|ctx| ctx.trace_id.clone()),
        record_count,
        claim_count,
        relation_count,
        episode_count,
        alias_count,
        evidence_count,
        status: status.into(),
        source_exported_at: Some(batch.source_exported_at.clone()),
        transformed_at: Some(batch.transformed_at.clone()),
        imported_at,
        source_run_id: batch
            .export_meta
            .as_ref()
            .and_then(|meta| meta.run_id.clone()),
        comparability_snapshot_version: batch
            .export_meta
            .as_ref()
            .and_then(|meta| meta.comparability_snapshot_version.clone()),
        direct_write: batch
            .export_meta
            .as_ref()
            .map(|meta| meta.direct_write)
            .unwrap_or(false),
        failure_reason,
        evidence_bundle_id,
        evidence_bundle_json,
        episode_bundle_id,
        episode_bundle_json,
        execution_context_json,
        kernel_payload_json,
    })
}

fn projection_identity_conflict_error(
    source_envelope_id: &str,
    content_digest: &str,
    record_kind: &str,
    record_id: &str,
    existing_source_envelope_id: &str,
) -> MemoryError {
    if existing_source_envelope_id == source_envelope_id {
        MemoryError::ImportMigrationRequired {
            source_envelope_id: source_envelope_id.to_string(),
            detail: format!(
                "incoming {record_kind} {record_id} already exists for source_envelope_id {source_envelope_id} but the import receipt does not match digest {content_digest}; this usually means a historical digest migration replay or projection_import_log drift. Repair or clear the import receipts instead of replaying the same authoritative rows"
            ),
        }
    } else {
        MemoryError::ImportInvalid {
            reason: format!(
                "incoming {record_kind} {record_id} would collide with existing imported data from source_envelope_id {existing_source_envelope_id}; refusing ambiguous overwrite"
            ),
        }
    }
}

fn check_projection_identity_conflicts(
    conn: &rusqlite::Connection,
    source_envelope_id: &str,
    content_digest: &str,
    claim_rows: &[projection_storage::ClaimVersionRow],
    relation_rows: &[projection_storage::RelationVersionRow],
) -> Result<(), MemoryError> {
    for cv in claim_rows {
        if let Some(existing_source_envelope_id) =
            projection_storage::claim_version_source_envelope(conn, &cv.claim_version_id)?
        {
            return Err(projection_identity_conflict_error(
                source_envelope_id,
                content_digest,
                "claim_version_id",
                &cv.claim_version_id,
                &existing_source_envelope_id,
            ));
        }
    }

    for rv in relation_rows {
        if let Some(existing_source_envelope_id) =
            projection_storage::relation_version_source_envelope(conn, &rv.relation_version_id)?
        {
            return Err(projection_identity_conflict_error(
                source_envelope_id,
                content_digest,
                "relation_version_id",
                &rv.relation_version_id,
                &existing_source_envelope_id,
            ));
        }
    }

    Ok(())
}

fn parse_optional_receipt_json(
    raw: Option<String>,
    field_name: &str,
    row_id: &str,
) -> Result<Option<serde_json::Value>, MemoryError> {
    raw.map(|value| {
        serde_json::from_str(&value).map_err(|err| MemoryError::CorruptData {
            table: "projection_import_receipts",
            row_id: row_id.to_string(),
            detail: format!("invalid {field_name}: {err}"),
        })
    })
    .transpose()
}

fn parse_rebuildable_kernel_batch_v3(
    kernel_payload_json: Option<&serde_json::Value>,
    table: &'static str,
    row_id: &str,
) -> Result<Option<ProjectionImportBatchV3>, MemoryError> {
    kernel_payload_json
        .cloned()
        .map(|value| {
            serde_json::from_value(value).map_err(|err| MemoryError::CorruptData {
                table,
                row_id: row_id.to_string(),
                detail: format!("invalid rebuildable kernel batch v3 receipt: {err}"),
            })
        })
        .transpose()
}

fn parse_batch_timestamp(
    value: Option<&str>,
    column: &str,
    row_kind: &str,
    row_id: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, MemoryError> {
    match value {
        Some(value) => chrono::DateTime::parse_from_rfc3339(value)
            .map(|t| Some(t.with_timezone(&chrono::Utc)))
            .map_err(|err| MemoryError::ImportInvalid {
                reason: format!(
                    "invalid {row_kind} {row_id} {column}: {value}; expected RFC3339 timestamp ({err})"
                ),
            }),
        None => Ok(None),
    }
}

fn parse_stored_timestamp(
    value: Option<&str>,
    table: &'static str,
    row_id: &str,
    column: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, MemoryError> {
    match value {
        Some(value) => chrono::DateTime::parse_from_rfc3339(value)
            .map(|t| Some(t.with_timezone(&chrono::Utc)))
            .map_err(|err| MemoryError::CorruptData {
                table,
                row_id: row_id.to_string(),
                detail: format!("invalid {column} timestamp '{value}' ({err})"),
            }),
        None => Ok(None),
    }
}

fn validate_temporal_order(
    row_kind: &str,
    row_id: &str,
    valid_from: Option<chrono::DateTime<chrono::Utc>>,
    valid_to: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<(), MemoryError> {
    if let (Some(from), Some(to)) = (valid_from, valid_to) {
        if from >= to {
            return Err(MemoryError::ImportInvalid {
                reason: format!(
                    "{row_kind} {row_id} has invalid interval: valid_from ({from}) is not < valid_to ({to})"
                ),
            });
        }
    }
    Ok(())
}

fn intervals_overlap(
    first_from: Option<chrono::DateTime<chrono::Utc>>,
    first_to: Option<chrono::DateTime<chrono::Utc>>,
    second_from: Option<chrono::DateTime<chrono::Utc>>,
    second_to: Option<chrono::DateTime<chrono::Utc>>,
) -> bool {
    if let (Some(a_to), Some(b_from)) = (first_to, second_from) {
        if b_from >= a_to {
            return false;
        }
    }
    if let (Some(a_from), Some(b_to)) = (first_from, second_to) {
        if a_from >= b_to {
            return false;
        }
    }
    true
}

/// Result of a projection batch import (V11+).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectionImportResult {
    /// Source envelope ID.
    pub source_envelope_id: String,
    /// Import status: "complete" or "already_imported".
    pub status: String,
    /// Number of records in the batch.
    pub record_count: usize,
    /// Whether this was a duplicate (idempotent no-op).
    pub was_duplicate: bool,
}

/// Public view of a V11 projection import log entry.
#[derive(Debug, Clone, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
#[schemars(title = "ProjectionImportLogEntryV1")]
pub struct ProjectionImportLogEntry {
    pub batch_id: String,
    pub source_envelope_id: String,
    /// Import-side batch schema version recorded at the memory boundary.
    pub schema_version: String,
    /// Source export schema version preserved as provenance when provided.
    pub export_schema_version: Option<String>,
    pub content_digest: String,
    pub source_authority: String,
    pub scope_namespace: String,
    pub scope_domain: Option<String>,
    pub scope_workspace_id: Option<String>,
    pub scope_repo_id: Option<String>,
    pub record_count: usize,
    pub claim_count: usize,
    pub relation_count: usize,
    pub episode_count: usize,
    pub alias_count: usize,
    pub evidence_count: usize,
    pub status: String,
    pub source_exported_at: Option<String>,
    pub transformed_at: Option<String>,
    pub imported_at: String,
    pub source_run_id: Option<String>,
    pub comparability_snapshot_version: Option<String>,
    pub direct_write: bool,
    pub failure_reason: Option<String>,
    pub evidence_bundle_id: Option<String>,
    pub evidence_bundle_json: Option<serde_json::Value>,
    pub episode_bundle_id: Option<String>,
    pub episode_bundle_json: Option<serde_json::Value>,
    pub execution_context_json: Option<serde_json::Value>,
    pub kernel_payload_json: Option<serde_json::Value>,
}

impl ProjectionImportLogEntry {
    pub fn scope_key(&self) -> ScopeKey {
        ScopeKey {
            namespace: self.scope_namespace.clone(),
            domain: self.scope_domain.clone(),
            workspace_id: self.scope_workspace_id.clone(),
            repo_id: self.scope_repo_id.clone(),
        }
    }

    pub fn rebuildable_kernel_batch_v3(
        &self,
    ) -> Result<Option<ProjectionImportBatchV3>, MemoryError> {
        parse_rebuildable_kernel_batch_v3(
            self.kernel_payload_json.as_ref(),
            "projection_import_log",
            &self.batch_id,
        )
    }
}

/// Public view of a durable failed projection import receipt.
#[derive(Debug, Clone, schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
#[schemars(title = "ImportFailureRecordV1")]
pub struct ProjectionImportFailureReceiptEntry {
    pub failure_id: String,
    pub source_envelope_id: String,
    pub schema_version: String,
    pub export_schema_version: Option<String>,
    pub content_digest: String,
    pub source_authority: String,
    pub scope_namespace: String,
    pub scope_domain: Option<String>,
    pub scope_workspace_id: Option<String>,
    pub scope_repo_id: Option<String>,
    pub record_count: usize,
    pub error_kind: String,
    pub error_message: String,
    pub source_exported_at: Option<String>,
    pub transformed_at: Option<String>,
    pub failed_at: String,
    pub source_run_id: Option<String>,
    pub comparability_snapshot_version: Option<String>,
    pub direct_write: bool,
    pub evidence_bundle_id: Option<String>,
    pub evidence_bundle_json: Option<serde_json::Value>,
    pub episode_bundle_id: Option<String>,
    pub episode_bundle_json: Option<serde_json::Value>,
    pub execution_context_json: Option<serde_json::Value>,
    pub kernel_payload_json: Option<serde_json::Value>,
}

impl ProjectionImportFailureReceiptEntry {
    pub fn scope_key(&self) -> ScopeKey {
        ScopeKey {
            namespace: self.scope_namespace.clone(),
            domain: self.scope_domain.clone(),
            workspace_id: self.scope_workspace_id.clone(),
            repo_id: self.scope_repo_id.clone(),
        }
    }

    pub fn rebuildable_kernel_batch_v3(
        &self,
    ) -> Result<Option<ProjectionImportBatchV3>, MemoryError> {
        parse_rebuildable_kernel_batch_v3(
            self.kernel_payload_json.as_ref(),
            "projection_import_failures",
            &self.failure_id,
        )
    }
}

fn projection_import_log_entry_from_row(
    r: projection_storage::ProjectionImportLogRow,
) -> Result<ProjectionImportLogEntry, MemoryError> {
    let row_id = r.batch_id.clone();
    Ok(ProjectionImportLogEntry {
        batch_id: r.batch_id,
        source_envelope_id: r.source_envelope_id,
        schema_version: r.schema_version,
        export_schema_version: r.export_schema_version,
        content_digest: r.content_digest,
        source_authority: r.source_authority,
        scope_namespace: r.scope_namespace,
        scope_domain: r.scope_domain,
        scope_workspace_id: r.scope_workspace_id,
        scope_repo_id: r.scope_repo_id,
        record_count: r.record_count,
        claim_count: r.claim_count,
        relation_count: r.relation_count,
        episode_count: r.episode_count,
        alias_count: r.alias_count,
        evidence_count: r.evidence_count,
        status: r.status,
        source_exported_at: r.source_exported_at,
        transformed_at: r.transformed_at,
        imported_at: r.imported_at,
        source_run_id: r.source_run_id,
        comparability_snapshot_version: r.comparability_snapshot_version,
        direct_write: r.direct_write,
        failure_reason: r.failure_reason,
        evidence_bundle_id: r.evidence_bundle_id,
        evidence_bundle_json: parse_optional_receipt_json(
            r.evidence_bundle_json,
            "evidence_bundle_json",
            &row_id,
        )?,
        episode_bundle_id: r.episode_bundle_id,
        episode_bundle_json: parse_optional_receipt_json(
            r.episode_bundle_json,
            "episode_bundle_json",
            &row_id,
        )?,
        execution_context_json: parse_optional_receipt_json(
            r.execution_context_json,
            "execution_context_json",
            &row_id,
        )?,
        kernel_payload_json: parse_optional_receipt_json(
            r.kernel_payload_json,
            "kernel_payload_json",
            &row_id,
        )?,
    })
}

fn projection_import_failure_entry_from_row(
    r: projection_storage::ProjectionImportFailureRow,
) -> Result<ProjectionImportFailureReceiptEntry, MemoryError> {
    let row_id = r.failure_id.clone();
    Ok(ProjectionImportFailureReceiptEntry {
        failure_id: r.failure_id,
        source_envelope_id: r.source_envelope_id,
        schema_version: r.schema_version,
        export_schema_version: r.export_schema_version,
        content_digest: r.content_digest,
        source_authority: r.source_authority,
        scope_namespace: r.scope_namespace,
        scope_domain: r.scope_domain,
        scope_workspace_id: r.scope_workspace_id,
        scope_repo_id: r.scope_repo_id,
        record_count: r.record_count,
        error_kind: r.error_kind,
        error_message: r.error_message,
        source_exported_at: r.source_exported_at,
        transformed_at: r.transformed_at,
        failed_at: r.failed_at,
        source_run_id: r.source_run_id,
        comparability_snapshot_version: r.comparability_snapshot_version,
        direct_write: r.direct_write,
        evidence_bundle_id: r.evidence_bundle_id,
        evidence_bundle_json: parse_optional_receipt_json(
            r.evidence_bundle_json,
            "evidence_bundle_json",
            &row_id,
        )?,
        episode_bundle_id: r.episode_bundle_id,
        episode_bundle_json: parse_optional_receipt_json(
            r.episode_bundle_json,
            "episode_bundle_json",
            &row_id,
        )?,
        execution_context_json: parse_optional_receipt_json(
            r.execution_context_json,
            "execution_context_json",
            &row_id,
        )?,
        kernel_payload_json: parse_optional_receipt_json(
            r.kernel_payload_json,
            "kernel_payload_json",
            &row_id,
        )?,
    })
}

impl MemoryStore {
    async fn persist_projection_import_failure_receipt(
        &self,
        log_row: projection_storage::ProjectionImportLogRow,
        error: &MemoryError,
    ) {
        let failed_log = projection_storage::ProjectionImportLogRow {
            status: "failed".into(),
            failure_reason: Some(error.to_string()),
            ..log_row.clone()
        };
        let failure_row = projection_storage::ProjectionImportFailureRow {
            failure_id: projection_import_failure_id(
                &failed_log.source_envelope_id,
                &failed_log.schema_version,
                &failed_log.content_digest,
            ),
            source_envelope_id: failed_log.source_envelope_id.clone(),
            schema_version: failed_log.schema_version.clone(),
            export_schema_version: failed_log.export_schema_version.clone(),
            content_digest: failed_log.content_digest.clone(),
            source_authority: failed_log.source_authority.clone(),
            scope_namespace: failed_log.scope_namespace.clone(),
            scope_domain: failed_log.scope_domain.clone(),
            scope_workspace_id: failed_log.scope_workspace_id.clone(),
            scope_repo_id: failed_log.scope_repo_id.clone(),
            trace_id: failed_log.trace_id.clone(),
            record_count: failed_log.record_count,
            error_kind: error.kind().into(),
            error_message: error.to_string(),
            source_exported_at: failed_log.source_exported_at.clone(),
            transformed_at: failed_log.transformed_at.clone(),
            failed_at: failed_log.imported_at.clone(),
            source_run_id: failed_log.source_run_id.clone(),
            comparability_snapshot_version: failed_log.comparability_snapshot_version.clone(),
            direct_write: failed_log.direct_write,
            evidence_bundle_id: failed_log.evidence_bundle_id.clone(),
            evidence_bundle_json: failed_log.evidence_bundle_json.clone(),
            episode_bundle_id: failed_log.episode_bundle_id.clone(),
            episode_bundle_json: failed_log.episode_bundle_json.clone(),
            execution_context_json: failed_log.execution_context_json.clone(),
            kernel_payload_json: failed_log.kernel_payload_json.clone(),
        };

        let result = self
            .with_write_conn(move |conn| {
                projection_storage::upsert_projection_import_log_conn(conn, &failed_log)?;
                projection_storage::insert_projection_import_failure(conn, &failure_row)?;
                Ok(())
            })
            .await;

        if let Err(log_error) = result {
            tracing::warn!(
                error = %log_error,
                "failed to persist projection import failure receipt"
            );
        }
    }

    /// Import a projection batch from `forge-memory-bridge`.
    ///
    /// This is the canonical in-process import path for the stack:
    /// `ExportEnvelopeV3 -> transform_envelope_v3() -> ProjectionImportBatchV3
    /// -> semantic-memory import transaction`.
    ///
    /// V2 remains supported as a compatibility-normalized import batch shape.
    /// The old `import_envelope()` method remains functional only during the
    /// migration cycle. JSON parsing is retained only via
    /// [`import_projection_batch_json_compat()`](Self::import_projection_batch_json_compat).
    pub async fn import_projection_batch<B: ProjectionImportBatchLike>(
        &self,
        batch: &B,
    ) -> Result<ProjectionImportResult, MemoryError> {
        let kernel_payload_json = batch.kernel_payload_json()?;
        let batch = batch.to_projection_import_batch_v2();

        if batch.schema_version != PROJECTION_IMPORT_BATCH_V2_SCHEMA {
            return Err(MemoryError::ImportInvalid {
                reason: format!(
                    "unsupported schema_version: {}; expected {}",
                    batch.schema_version, PROJECTION_IMPORT_BATCH_V2_SCHEMA
                ),
            });
        }

        let source_envelope_id = batch.source_envelope_id.as_str().to_string();
        let schema_version = batch.schema_version.clone();
        let export_schema_version = batch.export_schema_version.clone();
        let content_digest = batch.content_digest.hex().to_string();
        let source_authority = batch.source_authority.clone();
        let scope_namespace = batch.scope_key.namespace.clone();
        let scope_domain = batch.scope_key.domain.clone();
        let scope_workspace_id = batch.scope_key.workspace_id.clone();
        let scope_repo_id = batch.scope_key.repo_id.clone();
        let trace_id = batch.trace_ctx.as_ref().map(|ctx| ctx.trace_id.clone());
        let source_exported_at = Some(batch.source_exported_at.clone());
        let transformed_at = Some(batch.transformed_at.clone());
        let source_run_id = batch
            .export_meta
            .as_ref()
            .and_then(|meta| meta.run_id.clone());
        let comparability_snapshot_version = batch
            .export_meta
            .as_ref()
            .and_then(|meta| meta.comparability_snapshot_version.clone());
        let direct_write = batch
            .export_meta
            .as_ref()
            .map(|meta| meta.direct_write)
            .unwrap_or(false);
        let (evidence_bundle_id, evidence_bundle_json) = serialized_import_evidence_bundle(&batch)?;
        let (episode_bundle_id, episode_bundle_json) = serialized_episode_bundle(&batch)?;
        let execution_context_json = serialized_execution_context(&batch)?;
        let record_len = batch.records.len();
        let base_failure_log_row = projection_storage::ProjectionImportLogRow {
            batch_id: projection_import_failure_id(
                &source_envelope_id,
                &schema_version,
                &content_digest,
            ),
            source_envelope_id: source_envelope_id.clone(),
            schema_version: schema_version.clone(),
            export_schema_version: export_schema_version.clone(),
            content_digest: content_digest.clone(),
            source_authority: source_authority.clone(),
            scope_namespace: scope_namespace.clone(),
            scope_domain: scope_domain.clone(),
            scope_workspace_id: scope_workspace_id.clone(),
            scope_repo_id: scope_repo_id.clone(),
            trace_id: trace_id.clone(),
            record_count: record_len,
            claim_count: 0,
            relation_count: 0,
            episode_count: 0,
            alias_count: 0,
            evidence_count: 0,
            status: "failed".into(),
            source_exported_at: source_exported_at.clone(),
            transformed_at: transformed_at.clone(),
            imported_at: chrono::Utc::now().to_rfc3339(),
            source_run_id: source_run_id.clone(),
            comparability_snapshot_version: comparability_snapshot_version.clone(),
            direct_write,
            failure_reason: None,
            evidence_bundle_id,
            evidence_bundle_json,
            episode_bundle_id,
            episode_bundle_json,
            execution_context_json,
            kernel_payload_json: kernel_payload_json.clone(),
        };

        if batch.schema_version != PROJECTION_IMPORT_BATCH_V2_SCHEMA {
            let error = MemoryError::ImportInvalid {
                reason: format!(
                    "unsupported schema_version: {}; expected {}",
                    batch.schema_version, PROJECTION_IMPORT_BATCH_V2_SCHEMA
                ),
            };
            self.persist_projection_import_failure_receipt(base_failure_log_row, &error)
                .await;
            return Err(error);
        }

        let sei_c = source_envelope_id.clone();
        let sv_c = schema_version.clone();
        let cd_c = content_digest.clone();
        let already = self
            .with_read_conn(move |conn| {
                projection_storage::check_projection_import_exists(conn, &sei_c, &sv_c, &cd_c)
            })
            .await?;

        if already {
            return Ok(ProjectionImportResult {
                source_envelope_id,
                status: "already_imported".into(),
                record_count: record_len,
                was_duplicate: true,
            });
        }

        let batch_id = uuid::Uuid::new_v4().to_string();

        let mut claim_count = 0usize;
        let mut relation_count = 0usize;
        let mut episode_count = 0usize;
        let mut alias_count = 0usize;
        let mut evidence_count = 0usize;

        let mut claim_rows = Vec::new();
        let mut relation_rows = Vec::new();
        let mut alias_rows = Vec::new();
        let mut evidence_rows = Vec::new();
        let mut episode_rows = Vec::new();
        let mut preferred_claim_intervals = Vec::new();
        let mut preferred_relation_intervals = Vec::new();

        for record in &batch.records {
            match record {
                ImportProjectionRecord::ClaimVersion(cv) => {
                    let valid_from = parse_batch_timestamp(
                        cv.valid_from.as_deref(),
                        "valid_from",
                        "claim_version",
                        cv.claim_version_id.as_str(),
                    )?;
                    let valid_to = parse_batch_timestamp(
                        cv.valid_to.as_deref(),
                        "valid_to",
                        "claim_version",
                        cv.claim_version_id.as_str(),
                    )?;
                    validate_temporal_order(
                        "claim_version",
                        cv.claim_version_id.as_str(),
                        valid_from,
                        valid_to,
                    )?;

                    claim_count += 1;
                    claim_rows.push(projection_storage::ClaimVersionRow {
                        claim_version_id: cv.claim_version_id.as_str().to_string(),
                        claim_id: cv.claim_id.as_str().to_string(),
                        claim_state: cv.claim_state.as_str().to_string(),
                        projection_family: cv.projection_family.clone(),
                        subject_entity_id: cv.subject_entity_id.as_str().to_string(),
                        predicate: cv.predicate.clone(),
                        object_anchor: cv.object_anchor.to_string(),
                        scope_namespace: cv.scope_key.namespace.clone(),
                        scope_domain: cv.scope_key.domain.clone(),
                        scope_workspace_id: cv.scope_key.workspace_id.clone(),
                        scope_repo_id: cv.scope_key.repo_id.clone(),
                        valid_from: cv.valid_from.clone(),
                        valid_to: cv.valid_to.clone(),
                        recorded_at: String::new(),
                        preferred_open: cv.preferred_open,
                        source_envelope_id: cv.source_envelope_id.as_str().to_string(),
                        source_authority: cv.source_authority.clone(),
                        trace_id: cv.trace_ctx.as_ref().map(|ctx| ctx.trace_id.clone()),
                        freshness: cv.freshness.as_str().to_string(),
                        contradiction_status: serde_json::to_string(&cv.contradiction_status)
                            .unwrap_or_else(|_| "\"none\"".into()),
                        supersedes_claim_version_id: cv
                            .supersedes_claim_version_id
                            .as_ref()
                            .map(|id| id.as_str().to_string()),
                        content: cv.content.clone(),
                        confidence: cv.confidence,
                        content_digest: Some(batch.content_digest.hex().to_string()),
                        metadata: cv.metadata.as_ref().map(|v| v.to_string()),
                    });

                    if cv.preferred_open {
                        preferred_claim_intervals.push((
                            cv.claim_id.as_str().to_string(),
                            cv.claim_version_id.as_str().to_string(),
                            valid_from,
                            valid_to,
                        ));
                    }
                }
                ImportProjectionRecord::RelationVersion(rv) => {
                    let valid_from = parse_batch_timestamp(
                        rv.valid_from.as_deref(),
                        "valid_from",
                        "relation_version",
                        rv.relation_version_id.as_str(),
                    )?;
                    let valid_to = parse_batch_timestamp(
                        rv.valid_to.as_deref(),
                        "valid_to",
                        "relation_version",
                        rv.relation_version_id.as_str(),
                    )?;
                    validate_temporal_order(
                        "relation_version",
                        rv.relation_version_id.as_str(),
                        valid_from,
                        valid_to,
                    )?;

                    relation_count += 1;
                    relation_rows.push(projection_storage::RelationVersionRow {
                        relation_version_id: rv.relation_version_id.as_str().to_string(),
                        subject_entity_id: rv.subject_entity_id.as_str().to_string(),
                        predicate: rv.predicate.clone(),
                        object_anchor: rv.object_anchor.to_string(),
                        scope_namespace: rv.scope_key.namespace.clone(),
                        scope_domain: rv.scope_key.domain.clone(),
                        scope_workspace_id: rv.scope_key.workspace_id.clone(),
                        scope_repo_id: rv.scope_key.repo_id.clone(),
                        claim_id: rv.claim_id.as_ref().map(|id| id.as_str().to_string()),
                        source_episode_id: rv
                            .source_episode_id
                            .as_ref()
                            .map(|id| id.as_str().to_string()),
                        valid_from: rv.valid_from.clone(),
                        valid_to: rv.valid_to.clone(),
                        recorded_at: String::new(),
                        preferred_open: rv.preferred_open,
                        supersedes_relation_version_id: rv
                            .supersedes_relation_version_id
                            .as_ref()
                            .map(|id| id.as_str().to_string()),
                        contradiction_status: serde_json::to_string(&rv.contradiction_status)
                            .unwrap_or_else(|_| "\"none\"".into()),
                        source_confidence: rv.source_confidence,
                        projection_family: rv.projection_family.clone(),
                        source_envelope_id: rv.source_envelope_id.as_str().to_string(),
                        source_authority: rv.source_authority.clone(),
                        trace_id: rv.trace_ctx.as_ref().map(|ctx| ctx.trace_id.clone()),
                        freshness: rv.freshness.as_str().to_string(),
                        metadata: rv.metadata.as_ref().map(|v| v.to_string()),
                    });

                    if rv.preferred_open {
                        preferred_relation_intervals.push((
                            rv.subject_entity_id.as_str().to_string(),
                            rv.predicate.clone(),
                            rv.object_anchor.to_string(),
                            rv.scope_key.namespace.clone(),
                            rv.scope_key.domain.clone(),
                            rv.scope_key.workspace_id.clone(),
                            rv.scope_key.repo_id.clone(),
                            rv.projection_family.clone(),
                            rv.relation_version_id.as_str().to_string(),
                            valid_from,
                            valid_to,
                        ));
                    }
                }
                ImportProjectionRecord::EntityAlias(ea) => {
                    alias_count += 1;
                    alias_rows.push(projection_storage::EntityAliasRow {
                        canonical_entity_id: ea.canonical_entity_id.as_str().to_string(),
                        alias_text: ea.alias_text.clone(),
                        alias_source: ea.alias_source.clone(),
                        match_evidence: ea.match_evidence.as_ref().map(|v| v.to_string()),
                        confidence: ea.confidence,
                        merge_decision: encode_merge_decision(&ea.merge_decision),
                        scope_namespace: ea.scope.namespace.clone(),
                        scope_domain: ea.scope.domain.clone(),
                        scope_workspace_id: ea.scope.workspace_id.clone(),
                        scope_repo_id: ea.scope.repo_id.clone(),
                        review_state: encode_review_state(&ea.review_state),
                        is_human_confirmed: ea.is_human_confirmed,
                        is_human_confirmed_final: ea.is_human_confirmed_final,
                        superseded_by_entity_id: ea
                            .superseded_by_entity_id
                            .as_ref()
                            .map(|id| id.as_str().to_string()),
                        split_from_entity_id: ea
                            .split_from_entity_id
                            .as_ref()
                            .map(|id| id.as_str().to_string()),
                        source_envelope_id: ea.source_envelope_id.as_str().to_string(),
                        recorded_at: String::new(),
                    });
                }
                ImportProjectionRecord::EvidenceRef(er) => {
                    evidence_count += 1;
                    evidence_rows.push(projection_storage::EvidenceRefRow {
                        claim_id: er.claim_id.as_str().to_string(),
                        claim_version_id: er
                            .claim_version_id
                            .as_ref()
                            .map(|id| id.as_str().to_string()),
                        fetch_handle: er.fetch_handle.clone(),
                        source_authority: er.source_authority.clone(),
                        source_envelope_id: er.source_envelope_id.as_str().to_string(),
                        recorded_at: String::new(),
                        metadata: er.metadata.as_ref().map(|v| v.to_string()),
                    });
                }
                ImportProjectionRecord::Episode(ep) => {
                    episode_count += 1;
                    episode_rows.push(projection_storage::EpisodeLinkRow {
                        episode_id: ep.episode_id.as_str().to_string(),
                        document_id: ep.document_id.clone(),
                        cause_ids: serde_json::to_string(&ep.cause_ids)
                            .unwrap_or_else(|_| "[]".into()),
                        effect_type: ep.effect_type.clone(),
                        outcome: ep.outcome.clone(),
                        confidence: ep.confidence,
                        experiment_id: ep.experiment_id.clone(),
                        source_envelope_id: ep.source_envelope_id.as_str().to_string(),
                        source_authority: ep.source_authority.clone(),
                        trace_id: ep.trace_ctx.as_ref().map(|ctx| ctx.trace_id.clone()),
                        recorded_at: String::new(),
                        metadata: ep.metadata.as_ref().map(|v| v.to_string()),
                    });
                }
            }
        }

        for i in 0..preferred_claim_intervals.len() {
            let (left_claim_id, left_version_id, left_from, left_to) =
                &preferred_claim_intervals[i];
            for (right_claim_id, right_version_id, right_from, right_to) in
                preferred_claim_intervals.iter().skip(i + 1)
            {
                if left_claim_id != right_claim_id {
                    continue;
                }
                if intervals_overlap(*left_from, *left_to, *right_from, *right_to) {
                    let err = MemoryError::ImportInvalid {
                        reason: format!(
                            "preferred-open claim interval conflict for claim_id {left_claim_id}: \
                             {left_version_id} ({left_from:?}, {left_to:?}) overlaps \
                             {right_version_id} ({right_from:?}, {right_to:?})",
                        ),
                    };
                    let failure_log_row = build_projection_import_log_row(
                        &batch,
                        batch_id.clone(),
                        "failed",
                        chrono::Utc::now().to_rfc3339(),
                        Some(err.to_string()),
                        kernel_payload_json.clone(),
                        record_len,
                        claim_count,
                        relation_count,
                        episode_count,
                        alias_count,
                        evidence_count,
                    )?;
                    self.persist_projection_import_failure_receipt(failure_log_row, &err)
                        .await;
                    return Err(err);
                }
            }
        }

        for i in 0..preferred_relation_intervals.len() {
            let (
                left_subject_entity_id,
                left_predicate,
                left_object_anchor,
                left_scope_namespace,
                left_scope_domain,
                left_scope_workspace_id,
                left_scope_repo_id,
                left_projection_family,
                left_relation_version_id,
                left_from,
                left_to,
            ) = &preferred_relation_intervals[i];
            for (
                right_subject_entity_id,
                right_predicate,
                right_object_anchor,
                right_scope_namespace,
                right_scope_domain,
                right_scope_workspace_id,
                right_scope_repo_id,
                right_projection_family,
                right_relation_version_id,
                right_from,
                right_to,
            ) in preferred_relation_intervals.iter().skip(i + 1)
            {
                if left_subject_entity_id != right_subject_entity_id
                    || left_predicate != right_predicate
                    || left_object_anchor != right_object_anchor
                    || left_scope_namespace != right_scope_namespace
                    || left_scope_domain != right_scope_domain
                    || left_scope_workspace_id != right_scope_workspace_id
                    || left_scope_repo_id != right_scope_repo_id
                    || left_projection_family != right_projection_family
                {
                    continue;
                }

                if intervals_overlap(*left_from, *left_to, *right_from, *right_to) {
                    let err = MemoryError::ImportInvalid {
                        reason: format!(
                            "preferred-open relation interval conflict for relation key \
                             ({left_subject_entity_id}, {left_predicate}, {left_object_anchor}, \
                             {left_scope_namespace}/{left_scope_domain:?}/{left_scope_workspace_id:?}/{left_scope_repo_id:?}, \
                             {left_projection_family}): {left_relation_version_id} \
                             ({left_from:?}, {left_to:?}) overlaps {right_relation_version_id} \
                             ({right_from:?}, {right_to:?})",
                        ),
                    };
                    let failure_log_row = build_projection_import_log_row(
                        &batch,
                        batch_id.clone(),
                        "failed",
                        chrono::Utc::now().to_rfc3339(),
                        Some(err.to_string()),
                        kernel_payload_json.clone(),
                        record_len,
                        claim_count,
                        relation_count,
                        episode_count,
                        alias_count,
                        evidence_count,
                    )?;
                    self.persist_projection_import_failure_receipt(failure_log_row, &err)
                        .await;
                    return Err(err);
                }
            }
        }

        let total_count = record_len;
        let success_log_row = build_projection_import_log_row(
            &batch,
            batch_id.clone(),
            "complete",
            chrono::Utc::now().to_rfc3339(),
            None,
            kernel_payload_json,
            total_count,
            claim_count,
            relation_count,
            episode_count,
            alias_count,
            evidence_count,
        )?;

        let import_result = self
            .with_write_conn(move |conn| {
                let mut claim_rows = claim_rows;
                let mut relation_rows = relation_rows;
                let mut alias_rows = alias_rows;
                let mut evidence_rows = evidence_rows;
                let mut episode_rows = episode_rows;
                let preferred_claim_intervals = preferred_claim_intervals;
                let preferred_relation_intervals = preferred_relation_intervals;
                let mut log_row = success_log_row;
                db::with_transaction(conn, |tx| {
                    if projection_storage::check_projection_import_exists(
                        tx,
                        &source_envelope_id,
                        &schema_version,
                        &content_digest,
                    )? {
                        return Ok(ProjectionImportResult {
                            source_envelope_id,
                            status: "already_imported".into(),
                            record_count: total_count,
                            was_duplicate: true,
                        });
                    }

                    check_projection_identity_conflicts(
                        tx,
                        &source_envelope_id,
                        &content_digest,
                        &claim_rows,
                        &relation_rows,
                    )?;

                    let imported_at = chrono::Utc::now().to_rfc3339();
                    for cv in &mut claim_rows {
                        cv.recorded_at = imported_at.clone();
                    }
                    for rv in &mut relation_rows {
                        rv.recorded_at = imported_at.clone();
                    }
                    for ea in &mut alias_rows {
                        ea.recorded_at = imported_at.clone();
                    }
                    for er in &mut evidence_rows {
                        er.recorded_at = imported_at.clone();
                    }
                    for el in &mut episode_rows {
                        el.recorded_at = imported_at.clone();
                    }

                    log_row.imported_at = imported_at.clone();

                    for (claim_id, claim_version_id, interval_from, interval_to) in
                        preferred_claim_intervals.iter()
                    {
                        for (existing_claim_version_id, existing_valid_from, existing_valid_to) in
                            projection_storage::query_preferred_claim_intervals(tx, claim_id)?
                        {
                            let existing_valid_from = parse_stored_timestamp(
                                existing_valid_from.as_deref(),
                                "claim_versions",
                                &existing_claim_version_id,
                                "valid_from",
                            )?;
                            let existing_valid_to = parse_stored_timestamp(
                                existing_valid_to.as_deref(),
                                "claim_versions",
                                &existing_claim_version_id,
                                "valid_to",
                            )?;
                            if intervals_overlap(
                                *interval_from,
                                *interval_to,
                                existing_valid_from,
                                existing_valid_to,
                            ) {
                                return Err(MemoryError::ImportInvalid {
                                    reason: format!(
                                        "preferred-open claim interval conflict for claim_id {claim_id}: \
                                         incoming {claim_version_id} ({interval_from:?}, {interval_to:?}) \
                                         overlaps existing {existing_claim_version_id} \
                                         ({existing_valid_from:?}, {existing_valid_to:?})"
                                    ),
                                });
                            }
                        }
                    }

                    for (
                        subject_entity_id,
                        predicate,
                        object_anchor,
                        scope_namespace,
                        scope_domain,
                        scope_workspace_id,
                        scope_repo_id,
                        projection_family,
                        relation_version_id,
                        interval_from,
                        interval_to,
                    ) in preferred_relation_intervals.iter()
                    {
                        for (
                            existing_relation_version_id,
                            existing_valid_from,
                            existing_valid_to,
                        ) in projection_storage::query_preferred_relation_intervals(
                            tx,
                            subject_entity_id,
                            predicate,
                            object_anchor,
                            scope_namespace,
                            scope_domain.as_deref(),
                            scope_workspace_id.as_deref(),
                            scope_repo_id.as_deref(),
                            projection_family,
                        )? {
                            let existing_valid_from = parse_stored_timestamp(
                                existing_valid_from.as_deref(),
                                "relation_versions",
                                &existing_relation_version_id,
                                "valid_from",
                            )?;
                            let existing_valid_to = parse_stored_timestamp(
                                existing_valid_to.as_deref(),
                                "relation_versions",
                                &existing_relation_version_id,
                                "valid_to",
                            )?;
                            if intervals_overlap(
                                *interval_from,
                                *interval_to,
                                existing_valid_from,
                                existing_valid_to,
                            ) {
                                return Err(MemoryError::ImportInvalid {
                                    reason: format!(
                                        "preferred-open relation interval conflict for relation key \
                                         ({subject_entity_id}, {predicate}, {object_anchor}, \
                                         {scope_namespace}/{scope_domain:?}/{scope_workspace_id:?}/{scope_repo_id:?}, \
                                         {projection_family}): incoming {relation_version_id} \
                                         ({interval_from:?}, {interval_to:?}) overlaps existing \
                                         {existing_relation_version_id} ({existing_valid_from:?}, \
                                         {existing_valid_to:?})"
                                    ),
                                });
                            }
                        }
                    }

                    for cv in &claim_rows {
                        projection_storage::insert_claim_version(tx, cv)?;
                    }
                    for rv in &relation_rows {
                        projection_storage::insert_relation_version(tx, rv)?;
                    }
                    for ea in &alias_rows {
                        projection_storage::insert_entity_alias(tx, ea)?;
                    }
                    for er in &evidence_rows {
                        projection_storage::insert_evidence_ref(tx, er)?;
                    }
                    for el in &episode_rows {
                        projection_storage::insert_episode_link(tx, el)?;
                    }

                    let add_edge = |source_kind: &str,
                                    source_id: &str,
                                    target_kind: &str,
                                    target_id: &str,
                                    derivation_type: &str,
                                    invalidation_mode: &str| {
                        projection_storage::insert_derivation_edge(
                            tx,
                            source_kind,
                            source_id,
                            target_kind,
                            target_id,
                            derivation_type,
                            invalidation_mode,
                        )
                    };

                    for cv in &claim_rows {
                        add_edge(
                            "claim",
                            cv.claim_id.as_str(),
                            "claim_version",
                            cv.claim_version_id.as_str(),
                            "claim_version_of",
                            "on_source_change",
                        )?;

                        if let Some(previous_version_id) = &cv.supersedes_claim_version_id {
                            add_edge(
                                "claim_version",
                                previous_version_id.as_str(),
                                "claim_version",
                                cv.claim_version_id.as_str(),
                                "supersedes",
                                "on_supersession",
                            )?;
                        }
                    }

                    for rv in &relation_rows {
                        if let Some(claim_id) = &rv.claim_id {
                            add_edge(
                                "claim",
                                claim_id.as_str(),
                                "relation_version",
                                rv.relation_version_id.as_str(),
                                "supports_claim",
                                "on_source_change",
                            )?;
                        }

                        if let Some(episode_id) = &rv.source_episode_id {
                            add_edge(
                                "episode",
                                episode_id.as_str(),
                                "relation_version",
                                rv.relation_version_id.as_str(),
                                "supports_episode",
                                "on_source_change",
                            )?;
                        }

                        if let Some(previous_relation_id) = &rv.supersedes_relation_version_id {
                            add_edge(
                                "relation_version",
                                previous_relation_id.as_str(),
                                "relation_version",
                                rv.relation_version_id.as_str(),
                                "supersedes",
                                "on_supersession",
                            )?;
                        }
                    }

                    for ea in &alias_rows {
                        add_edge(
                            "entity",
                            ea.canonical_entity_id.as_str(),
                            "entity_alias",
                            ea.canonical_entity_id.as_str(),
                            "canonical_alias",
                            "on_alias_split",
                        )?;

                        if let Some(split_from_entity_id) = &ea.split_from_entity_id {
                            add_edge(
                                "entity",
                                split_from_entity_id.as_str(),
                                "entity_alias",
                                ea.canonical_entity_id.as_str(),
                                "alias_split_from",
                                "on_alias_split",
                            )?;
                        }

                        if let Some(superseded_by_entity_id) = &ea.superseded_by_entity_id {
                            add_edge(
                                "entity",
                                superseded_by_entity_id.as_str(),
                                "entity_alias",
                                ea.canonical_entity_id.as_str(),
                                "alias_superseded_by",
                                "on_supersession",
                            )?;
                        }
                    }

                    for er in &evidence_rows {
                        projection_storage::insert_derivation_edge(
                            tx,
                            "claim",
                            er.claim_id.as_str(),
                            "evidence_ref",
                            &er.fetch_handle,
                            "supports",
                            "on_source_change",
                        )?;

                        if let Some(cvid) = &er.claim_version_id {
                            projection_storage::insert_derivation_edge(
                                tx,
                                "claim_version",
                                cvid.as_str(),
                                "evidence_ref",
                                &er.fetch_handle,
                                "supports",
                                "on_source_change",
                            )?;
                        }
                    }

                    for el in &episode_rows {
                        let cause_ids: Vec<String> =
                            serde_json::from_str(&el.cause_ids).map_err(|err| {
                                MemoryError::ImportInvalid {
                                    reason: format!(
                                        "episode {} has invalid cause_ids JSON: {}",
                                        el.episode_id, err
                                    ),
                                }
                            })?;

                        for cause_id in &cause_ids {
                            add_edge(
                                "claim",
                                cause_id,
                                "episode",
                                el.episode_id.as_str(),
                                "caused_by_claim",
                                "on_source_change",
                            )?;
                        }
                    }

                    projection_storage::insert_projection_import_log(tx, &log_row)?;

                    Ok(ProjectionImportResult {
                        source_envelope_id,
                        status: "complete".into(),
                        record_count: total_count,
                        was_duplicate: false,
                    })
                })
            })
            .await;

        if let Err(ref error) = import_result {
            self.persist_projection_import_failure_receipt(base_failure_log_row, error)
                .await;
        }

        import_result
    }

    /// Deserialize and import a projection batch from JSON.
    ///
    /// This is a compatibility boundary for callers that still cross the
    /// in-process seam as serialized JSON. New code should pass
    /// `ProjectionImportBatchV3` directly to `import_projection_batch()`.
    pub async fn import_projection_batch_json_compat(
        &self,
        batch_json: &str,
    ) -> Result<ProjectionImportResult, MemoryError> {
        let batch = match json_compat_import::decode_projection_batch_json_compat(batch_json) {
            Ok(batch) => batch,
            Err(error) => {
                self.persist_projection_import_failure_receipt(
                    json_compat_import::build_json_compat_failure_log_row(
                        batch_json,
                        chrono::Utc::now().to_rfc3339(),
                    ),
                    &error,
                )
                .await;
                return Err(error);
            }
        };
        self.import_projection_batch(&batch).await
    }

    /// Query the V11 projection import log.
    pub async fn query_projection_imports(
        &self,
        scope_namespace: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ProjectionImportLogEntry>, MemoryError> {
        let ns = scope_namespace.map(|s| s.to_string());
        self.with_read_conn(move |conn| {
            let rows = projection_storage::query_projection_import_log(conn, ns.as_deref(), limit)?;
            rows.into_iter()
                .map(projection_import_log_entry_from_row)
                .collect::<Result<Vec<_>, MemoryError>>()
        })
        .await
    }

    /// Return the most recent exact-scope import receipt carrying a rebuildable
    /// kernel V3 batch.
    pub async fn latest_rebuildable_kernel_projection_import_for_scope(
        &self,
        scope_key: &ScopeKey,
    ) -> Result<Option<ProjectionImportLogEntry>, MemoryError> {
        let scope_key = scope_key.clone();
        self.with_read_conn(move |conn| {
            projection_storage::latest_rebuildable_kernel_projection_import(conn, &scope_key)?
                .map(projection_import_log_entry_from_row)
                .transpose()
        })
        .await
    }

    /// Query durable failed projection import receipts.
    pub async fn query_projection_import_failures(
        &self,
        scope_namespace: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ProjectionImportFailureReceiptEntry>, MemoryError> {
        let ns = scope_namespace.map(|s| s.to_string());
        self.with_read_conn(move |conn| {
            let rows =
                projection_storage::query_projection_import_failures(conn, ns.as_deref(), limit)?;
            rows.into_iter()
                .map(projection_import_failure_entry_from_row)
                .collect::<Result<Vec<_>, MemoryError>>()
        })
        .await
    }
}
