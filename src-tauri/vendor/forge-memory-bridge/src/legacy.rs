//! Legacy compatibility shim for the old ImportEnvelope path.
//!
//! This module provides `LegacyImportEnvelopeV1`, a type alias and converter
//! for the old `import_envelope()` path in semantic-memory. It exists solely
//! to support one migration cycle of backward compatibility.
//!
//! ## Phase status: compatibility / migration-only
//!
//! This module will be removed after the migration cycle completes.
//! New code must use `ExportEnvelopeV3` → `transform_envelope_v3()` →
//! `ProjectionImportBatchV3` instead.

use semantic_memory_forge::{
    ExportClaim, ExportEnvelopeV1, ExportEpisode, ExportRecord, EXPORT_ENVELOPE_V1_SCHEMA,
};
use serde::{Deserialize, Serialize};
use stack_ids::{EnvelopeId, EpisodeId, ScopeKey, TraceCtx};

use crate::batch::*;
use crate::error::BridgeError;

/// Legacy import envelope — compatibility wrapper for the old path.
///
/// This maps to the old `semantic-memory::projection_import::ImportEnvelope`
/// format. It is kept for one migration cycle only.
///
/// ## Phase status: compatibility / migration-only
/// Removal condition: remove when all consumers have migrated to `ExportEnvelopeV3` -> `transform_envelope_v3()` -> `ProjectionImportBatchV3`
#[deprecated(
    since = "0.1.0",
    note = "Legacy import envelope compatibility type is migration-only. Use ExportEnvelopeV3 -> transform_envelope_v3() -> ProjectionImportBatchV3."
)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyImportEnvelopeV1 {
    pub envelope_id: EnvelopeId,
    pub schema_version: String,
    pub content_digest: String,
    pub source_authority: String,
    pub trace_id: Option<String>,
    pub namespace: String,
    pub records: Vec<LegacyImportRecord>,
}

/// Legacy import record (Fact or Episode).
///
/// ## Phase status: compatibility / migration-only
/// Removal condition: remove when all consumers have migrated to canonical export records
#[deprecated(
    since = "0.1.0",
    note = "Legacy import record is migration-only. Use canonical export records instead."
)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LegacyImportRecord {
    Fact {
        content: String,
        source: Option<String>,
        metadata: Option<serde_json::Value>,
    },
    Episode {
        document_id: String,
        meta: LegacyEpisodeMeta,
    },
}

/// Legacy episode metadata.
///
/// ## Phase status: compatibility / migration-only
/// Removal condition: remove when all consumers have migrated to canonical export records
#[deprecated(
    since = "0.1.0",
    note = "Legacy episode metadata is migration-only and kept for compatibility only."
)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyEpisodeMeta {
    pub cause_ids: Vec<String>,
    pub effect_type: String,
    pub outcome: String,
    pub confidence: f32,
    pub experiment_id: Option<String>,
}

/// Convert a legacy import envelope into the new ExportEnvelopeV1 format.
///
/// This enables old callers to continue using the legacy path while the
/// bridge internally upgrades to the canonical format.
///
/// ## Phase status: compatibility / migration-only
/// Removal condition: remove when all consumers have migrated to `ExportEnvelopeV3` and `transform_envelope_v3()`
#[deprecated(
    since = "0.1.0",
    note = "Legacy conversion is migration-only. Prefer ExportEnvelopeV3 and transform_envelope_v3() on the canonical path."
)]
pub fn upgrade_legacy_envelope(
    legacy: &LegacyImportEnvelopeV1,
) -> Result<ExportEnvelopeV1, BridgeError> {
    let scope_key = ScopeKey::from_legacy_namespace(&legacy.namespace);

    let records: Vec<ExportRecord> = legacy
        .records
        .iter()
        .map(|r| match r {
            LegacyImportRecord::Fact {
                content,
                source,
                metadata,
            } => ExportRecord::Claim(ExportClaim {
                claim_id: None,
                claim_version_id: None,
                subject_entity_id: stack_ids::EntityId::new("_legacy_unresolved"),
                predicate: "legacy_fact".into(),
                object_anchor: serde_json::json!(content),
                valid_from: None,
                valid_to: None,
                confidence: 1.0,
                content: content.clone(),
                projection_family: "legacy_import".into(),
                supersedes_claim_id: None,
                supersedes_claim_version_id: None,
                metadata: {
                    let mut meta = metadata.clone().unwrap_or(serde_json::json!({}));
                    if let Some(src) = source {
                        meta.as_object_mut()
                            .map(|m| m.insert("_legacy_source".into(), serde_json::json!(src)));
                    }
                    Some(meta)
                },
            }),
            LegacyImportRecord::Episode { document_id, meta } => {
                ExportRecord::Episode(ExportEpisode {
                    episode_id: Some(EpisodeId::generate()),
                    document_id: document_id.clone(),
                    cause_ids: meta.cause_ids.clone(),
                    effect_type: meta.effect_type.clone(),
                    outcome: meta.outcome.clone(),
                    confidence: meta.confidence,
                    experiment_id: meta.experiment_id.clone(),
                    metadata: None,
                })
            }
        })
        .collect();

    let digest = ExportEnvelopeV1::compute_digest(&legacy.source_authority, &scope_key, &records)?;

    let trace_ctx = legacy
        .trace_id
        .as_ref()
        .map(|id| TraceCtx::from_legacy_trace_id(id.as_str()));

    Ok(ExportEnvelopeV1 {
        envelope_id: legacy.envelope_id.clone(),
        schema_version: EXPORT_ENVELOPE_V1_SCHEMA.into(),
        content_digest: digest,
        source_authority: legacy.source_authority.clone(),
        scope_key,
        trace_ctx,
        exported_at: chrono::Utc::now().to_rfc3339(),
        records,
    })
}

/// Convert a legacy import envelope directly into a ProjectionImportBatchV1.
///
/// Convenience function that combines upgrade + transform.
///
/// ## Phase status: compatibility / migration-only
/// Removal condition: remove when all consumers have migrated to `ExportEnvelopeV3` and `transform_envelope_v3()`
#[deprecated(
    since = "0.1.0",
    note = "Legacy conversion is migration-only. Prefer ExportEnvelopeV3 and transform_envelope_v3() on the canonical path."
)]
pub fn transform_legacy_envelope(
    legacy: &LegacyImportEnvelopeV1,
) -> Result<ProjectionImportBatchV1, BridgeError> {
    let upgraded = upgrade_legacy_envelope(legacy)?;
    crate::transform::transform_envelope(&upgraded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_legacy_envelope() -> LegacyImportEnvelopeV1 {
        LegacyImportEnvelopeV1 {
            envelope_id: EnvelopeId::new("legacy-001"),
            schema_version: "1.0".into(),
            content_digest: "old-digest".into(),
            source_authority: "forge".into(),
            trace_id: Some("trace-abc".into()),
            namespace: "test-ns".into(),
            records: vec![
                LegacyImportRecord::Fact {
                    content: "Rust is a systems language".into(),
                    source: Some("docs".into()),
                    metadata: None,
                },
                LegacyImportRecord::Episode {
                    document_id: "doc-1".into(),
                    meta: LegacyEpisodeMeta {
                        cause_ids: vec!["cause-1".into()],
                        effect_type: "test_failure".into(),
                        outcome: "confirmed".into(),
                        confidence: 0.9,
                        experiment_id: Some("exp-1".into()),
                    },
                },
            ],
        }
    }

    #[test]
    fn upgrade_legacy_envelope_succeeds() {
        let legacy = make_legacy_envelope();
        let upgraded = upgrade_legacy_envelope(&legacy).unwrap();

        assert_eq!(upgraded.envelope_id, legacy.envelope_id);
        assert_eq!(upgraded.schema_version, EXPORT_ENVELOPE_V1_SCHEMA);
        assert_eq!(upgraded.source_authority, "forge");
        assert_eq!(upgraded.scope_key.namespace, "test-ns");
        assert!(upgraded.scope_key.is_namespace_only());
        assert_eq!(upgraded.records.len(), 2);
        assert!(upgraded.trace_ctx.is_some());
        assert_eq!(upgraded.trace_ctx.as_ref().unwrap().trace_id, "trace-abc");
    }

    #[test]
    fn upgrade_preserves_fact_content_and_source() {
        let legacy = make_legacy_envelope();
        let upgraded = upgrade_legacy_envelope(&legacy).unwrap();

        match &upgraded.records[0] {
            ExportRecord::Claim(c) => {
                assert_eq!(c.content, "Rust is a systems language");
                assert_eq!(c.projection_family, "legacy_import");
                let meta = c.metadata.as_ref().unwrap();
                assert_eq!(meta["_legacy_source"], "docs");
            }
            _ => panic!("expected Claim"),
        }
    }

    #[test]
    fn upgrade_preserves_episode() {
        let legacy = make_legacy_envelope();
        let upgraded = upgrade_legacy_envelope(&legacy).unwrap();

        match &upgraded.records[1] {
            ExportRecord::Episode(ep) => {
                assert_eq!(ep.document_id, "doc-1");
                assert_eq!(ep.effect_type, "test_failure");
                assert_eq!(ep.outcome, "confirmed");
                assert_eq!(ep.cause_ids, vec!["cause-1"]);
            }
            _ => panic!("expected Episode"),
        }
    }

    #[test]
    fn transform_legacy_envelope_produces_batch() {
        let legacy = make_legacy_envelope();
        let batch = transform_legacy_envelope(&legacy).unwrap();

        assert_eq!(batch.source_envelope_id, legacy.envelope_id);
        assert_eq!(batch.records.len(), 2);
        assert!(matches!(
            &batch.records[0],
            ImportProjectionRecord::ClaimVersion(_)
        ));
        assert!(matches!(
            &batch.records[1],
            ImportProjectionRecord::Episode(_)
        ));
    }

    #[test]
    fn namespace_to_scope_key_mapping_is_deterministic() {
        let sk1 = ScopeKey::from_legacy_namespace("my-ns");
        let sk2 = ScopeKey::from_legacy_namespace("my-ns");
        assert_eq!(sk1, sk2);
        assert_eq!(sk1.to_legacy_namespace(), "my-ns");
    }

    #[test]
    fn backfill_preserves_provenance() {
        let legacy = make_legacy_envelope();
        let batch = transform_legacy_envelope(&legacy).unwrap();

        match &batch.records[0] {
            ImportProjectionRecord::ClaimVersion(cv) => {
                assert_eq!(cv.source_envelope_id, legacy.envelope_id);
                assert_eq!(cv.source_authority, "forge");
                assert_eq!(cv.projection_family, "legacy_import");
            }
            _ => panic!("expected ClaimVersion"),
        }
    }
}
