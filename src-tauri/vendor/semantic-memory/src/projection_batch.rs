//! Projection batch import seams and canonicalized batch normalization helpers.

use crate::error::MemoryError;
use forge_memory_bridge::{
    ImportProjectionRecord, ImportProjectionRecordV3, MergeDecision, ProjectionImportBatchV1,
    ProjectionImportBatchV2, ProjectionImportBatchV3, ReviewState,
    PROJECTION_IMPORT_BATCH_V2_SCHEMA,
};

pub(crate) const KERNEL_SEMANTICS_V3_METADATA_KEY: &str = "kernel_semantics_v3";

/// Compatibility bridge for legacy projection import shapes.
#[doc(hidden)]
pub trait ProjectionImportBatchLike {
    fn to_projection_import_batch_v2(&self) -> ProjectionImportBatchV2;

    fn kernel_payload_json(&self) -> Result<Option<String>, MemoryError> {
        Ok(None)
    }
}

impl ProjectionImportBatchLike for ProjectionImportBatchV1 {
    fn to_projection_import_batch_v2(&self) -> ProjectionImportBatchV2 {
        self.clone().into()
    }
}

impl ProjectionImportBatchLike for ProjectionImportBatchV2 {
    fn to_projection_import_batch_v2(&self) -> ProjectionImportBatchV2 {
        self.clone()
    }
}

impl ProjectionImportBatchLike for ProjectionImportBatchV3 {
    fn to_projection_import_batch_v2(&self) -> ProjectionImportBatchV2 {
        ProjectionImportBatchV2 {
            source_envelope_id: self.source_envelope_id.clone(),
            schema_version: PROJECTION_IMPORT_BATCH_V2_SCHEMA.into(),
            export_schema_version: self.export_schema_version.clone(),
            content_digest: self.content_digest.clone(),
            source_authority: self.source_authority.clone(),
            scope_key: self.scope_key.clone(),
            trace_ctx: self.trace_ctx.clone(),
            source_exported_at: self.source_exported_at.clone(),
            transformed_at: self.transformed_at.clone(),
            export_meta: self.export_meta.clone(),
            evidence_bundle: self.evidence_bundle.clone(),
            episode_bundle: self.episode_bundle.clone(),
            execution_context: self.execution_context.clone(),
            records: self
                .records
                .iter()
                .cloned()
                .map(normalize_v3_record_for_storage)
                .collect(),
        }
    }

    fn kernel_payload_json(&self) -> Result<Option<String>, MemoryError> {
        serde_json::to_string(self).map(Some).map_err(|err| {
            MemoryError::Other(format!("failed to serialize kernel V3 batch: {err}"))
        })
    }
}

pub(crate) fn normalize_v3_record_for_storage(
    record: ImportProjectionRecordV3,
) -> ImportProjectionRecord {
    let semantics_json = record
        .semantics
        .and_then(|semantics| serde_json::to_value(semantics).ok());

    match record.record {
        forge_memory_bridge::ImportProjectionRecord::ClaimVersion(mut claim) => {
            if let Some(semantics) = semantics_json.clone() {
                claim.metadata = Some(merge_metadata_with_kernel_semantics(
                    claim.metadata.take(),
                    semantics,
                ));
            }
            forge_memory_bridge::ImportProjectionRecord::ClaimVersion(claim)
        }
        forge_memory_bridge::ImportProjectionRecord::RelationVersion(mut relation) => {
            if let Some(semantics) = semantics_json.clone() {
                relation.metadata = Some(merge_metadata_with_kernel_semantics(
                    relation.metadata.take(),
                    semantics,
                ));
            }
            forge_memory_bridge::ImportProjectionRecord::RelationVersion(relation)
        }
        forge_memory_bridge::ImportProjectionRecord::Episode(mut episode) => {
            if let Some(semantics) = semantics_json.clone() {
                episode.metadata = Some(merge_metadata_with_kernel_semantics(
                    episode.metadata.take(),
                    semantics,
                ));
            }
            forge_memory_bridge::ImportProjectionRecord::Episode(episode)
        }
        forge_memory_bridge::ImportProjectionRecord::EvidenceRef(mut evidence) => {
            if let Some(semantics) = semantics_json {
                evidence.metadata = Some(merge_metadata_with_kernel_semantics(
                    evidence.metadata.take(),
                    semantics,
                ));
            }
            forge_memory_bridge::ImportProjectionRecord::EvidenceRef(evidence)
        }
        forge_memory_bridge::ImportProjectionRecord::EntityAlias(alias) => {
            forge_memory_bridge::ImportProjectionRecord::EntityAlias(alias)
        }
    }
}

pub(crate) fn merge_metadata_with_kernel_semantics(
    metadata: Option<serde_json::Value>,
    semantics: serde_json::Value,
) -> serde_json::Value {
    match metadata {
        Some(serde_json::Value::Object(mut object)) => {
            object.insert(KERNEL_SEMANTICS_V3_METADATA_KEY.to_string(), semantics);
            serde_json::Value::Object(object)
        }
        Some(other) => serde_json::json!({
            "legacy_metadata": other,
            KERNEL_SEMANTICS_V3_METADATA_KEY: semantics,
        }),
        None => serde_json::json!({
            KERNEL_SEMANTICS_V3_METADATA_KEY: semantics,
        }),
    }
}

pub(crate) fn encode_merge_decision(decision: &MergeDecision) -> String {
    match decision {
        MergeDecision::PendingReview => "pending_review".into(),
        _ => serde_json::to_string(decision).unwrap_or_else(|_| "\"pending_review\"".into()),
    }
}

pub(crate) fn encode_review_state(state: &ReviewState) -> String {
    match state {
        ReviewState::Unreviewed => "unreviewed".into(),
        ReviewState::PendingReview => "pending_review".into(),
        _ => serde_json::to_string(state).unwrap_or_else(|_| "\"unreviewed\"".into()),
    }
}
