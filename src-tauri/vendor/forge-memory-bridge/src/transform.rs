#![allow(deprecated)]

//! Transformation from export envelopes to projection import batches.
//!
//! The bridge consumes validated export envelopes and produces import batches.
//! It preserves envelope ID, content digest, lineage, receipt refs, and trace
//! metadata. It rejects malformed exports before memory writes begin.
//!
//! ## Phase status: current / implemented now

use crate::batch::*;
use crate::error::BridgeError;
use semantic_memory_forge::{
    DispatchOutcomeV1, EpisodeBundleV1, ExecutionContextV1, ExportEnvelopeV1, ExportEnvelopeV2,
    ExportEnvelopeV3, ExportRecord, ExportRecordV3, EXPORT_ENVELOPE_V1_SCHEMA,
    EXPORT_ENVELOPE_V2_SCHEMA, EXPORT_ENVELOPE_V3_SCHEMA,
};
use stack_ids::{ClaimId, ClaimVersionId, RelationVersionId, TraceCtx};

/// Transform an `ExportEnvelopeV1` into a `ProjectionImportBatchV1`.
///
/// This is a compatibility-only bridge operation. It:
/// 1. Validates the export envelope (structure + digest).
/// 2. Transforms each record into the import projection format.
/// 3. Assigns version IDs where the source did not provide them.
/// 4. Preserves provenance throughout.
///
/// New integrations should use [`transform_envelope_v3()`].
///
/// ## Claim supersession lineage
///
/// When the export carries `supersedes_claim_version_id`, the bridge preserves
/// it verbatim. If only claim-level supersession is present, the bridge leaves
/// `ImportClaimVersion::supersedes_claim_version_id` empty. No synthetic values
/// are minted.
///
/// Returns an error if the envelope is malformed or incompatible.
///
/// Phase status: migration-only
/// Removal condition: remove when all consumers have migrated to `transform_envelope_v3()` and `ProjectionImportBatchV3`
#[deprecated(
    since = "0.2.0",
    note = "transform_envelope() is compatibility-only. Use transform_envelope_v3() and ProjectionImportBatchV3."
)]
pub fn transform_envelope(
    envelope: &ExportEnvelopeV1,
) -> Result<ProjectionImportBatchV1, BridgeError> {
    // Step 1: Validate
    envelope.validate()?;

    let now = chrono::Utc::now().to_rfc3339();

    // Step 2: Transform records
    let records = envelope
        .records
        .iter()
        .map(|record| transform_record(record, envelope))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ProjectionImportBatchV1 {
        source_envelope_id: envelope.envelope_id.clone(),
        schema_version: PROJECTION_IMPORT_BATCH_V1_SCHEMA.into(),
        export_schema_version: Some(envelope.schema_version.clone()),
        content_digest: envelope.content_digest.clone(),
        source_authority: envelope.source_authority.clone(),
        scope_key: envelope.scope_key.clone(),
        trace_ctx: envelope.trace_ctx.clone(),
        source_exported_at: envelope.exported_at.clone(),
        transformed_at: now,
        records,
    })
}

/// Transform an `ExportEnvelopeV2` into a `ProjectionImportBatchV2`.
///
/// This is compatibility-only. It preserves schema-level metadata from `V2`
/// exports into a `V2` projection batch, but it is not the normal integration
/// path. New integrations should upgrade through `ExportEnvelopeV3` and use
/// `transform_envelope_v3()`.
///
/// V2 preserves export-time metadata alongside the typed projection records.
/// The bridge copies it through unchanged and still does not invent meaning.
///
/// Phase status: migration-only
/// Removal condition: remove when all consumers have migrated to `transform_envelope_v3()` and `ProjectionImportBatchV3`
#[deprecated(
    since = "0.2.0",
    note = "transform_envelope_v2() is compatibility-only. Use transform_envelope_v3() and ProjectionImportBatchV3."
)]
pub fn transform_envelope_v2(
    envelope: &ExportEnvelopeV2,
) -> Result<ProjectionImportBatchV2, BridgeError> {
    envelope.validate()?;

    let now = chrono::Utc::now().to_rfc3339();
    let records = envelope
        .records
        .iter()
        .map(|record| transform_record_v2(record, envelope))
        .collect::<Result<Vec<_>, _>>()?;

    let execution_context = derive_execution_context_v2(envelope);
    let episode_bundle = derive_episode_bundle_v2(envelope, &execution_context)?;

    Ok(ProjectionImportBatchV2 {
        source_envelope_id: envelope.envelope_id.clone(),
        schema_version: PROJECTION_IMPORT_BATCH_V2_SCHEMA.into(),
        export_schema_version: Some(envelope.schema_version.clone()),
        content_digest: envelope.content_digest.clone(),
        source_authority: envelope.source_authority.clone(),
        scope_key: envelope.scope_key.clone(),
        trace_ctx: envelope.trace_ctx.clone(),
        source_exported_at: envelope.exported_at.clone(),
        transformed_at: now,
        export_meta: envelope.export_meta.clone(),
        evidence_bundle: envelope.evidence_bundle.clone(),
        episode_bundle,
        execution_context: Some(execution_context),
        records,
    })
}

/// Transform an `ExportEnvelopeV3` into a `ProjectionImportBatchV3`.
///
/// V3 is the kernel-oriented path. The bridge preserves the richer record
/// semantics exactly as exported and still does not invent missing lineage,
/// causal roles, or contradiction grouping.
pub fn transform_envelope_v3(
    envelope: &ExportEnvelopeV3,
) -> Result<ProjectionImportBatchV3, BridgeError> {
    envelope.validate()?;

    let now = chrono::Utc::now().to_rfc3339();
    let records = envelope
        .records
        .iter()
        .map(|record| transform_record_v3(record, envelope))
        .collect::<Result<Vec<_>, _>>()?;

    let execution_context = derive_execution_context_v3(envelope);
    let episode_bundle = derive_episode_bundle_v3(envelope, &execution_context)?;

    Ok(ProjectionImportBatchV3 {
        source_envelope_id: envelope.envelope_id.clone(),
        schema_version: PROJECTION_IMPORT_BATCH_V3_SCHEMA.into(),
        export_schema_version: Some(envelope.schema_version.clone()),
        content_digest: envelope.content_digest.clone(),
        source_authority: envelope.source_authority.clone(),
        scope_key: envelope.scope_key.clone(),
        trace_ctx: envelope.trace_ctx.clone(),
        source_exported_at: envelope.exported_at.clone(),
        transformed_at: now,
        export_meta: envelope.export_meta.clone(),
        evidence_bundle: envelope.evidence_bundle.clone(),
        episode_bundle,
        execution_context: Some(execution_context),
        support_sets: envelope.support_sets.clone(),
        contradiction_witnesses: envelope.contradiction_witnesses.clone(),
        retraction_records: envelope.retraction_records.clone(),
        claim_states_v13: envelope.claim_states_v13.clone(),
        intervention_bundles_v14: envelope.intervention_bundles_v14.clone(),
        outcome_schemas_v14: envelope.outcome_schemas_v14.clone(),
        cohort_contracts_v14: envelope.cohort_contracts_v14.clone(),
        counterfactual_slices_v14: envelope.counterfactual_slices_v14.clone(),
        experiment_cases_v14: envelope.experiment_cases_v14.clone(),
        comparability_matrices_v14: envelope.comparability_matrices_v14.clone(),
        decision_traces_v14: envelope.decision_traces_v14.clone(),
        refuter_suites_v14: envelope.refuter_suites_v14.clone(),
        refuter_results_v14: envelope.refuter_results_v14.clone(),
        experiment_budgets_v14: envelope.experiment_budgets_v14.clone(),
        rollout_decisions_v14: envelope.rollout_decisions_v14.clone(),
        rollback_decisions_v14: envelope.rollback_decisions_v14.clone(),
        attestation_envelopes_v15: envelope.attestation_envelopes_v15.clone(),
        trust_root_sets_v15: envelope.trust_root_sets_v15.clone(),
        artifact_admission_policies_v15: envelope.artifact_admission_policies_v15.clone(),
        transparency_receipts_v15: envelope.transparency_receipts_v15.clone(),
        attestation_revocations_v15: envelope.attestation_revocations_v15.clone(),
        attestation_supersessions_v15: envelope.attestation_supersessions_v15.clone(),
        remote_oracle_leases_v15: envelope.remote_oracle_leases_v15.clone(),
        remote_slice_requests_v15: envelope.remote_slice_requests_v15.clone(),
        remote_slice_results_v15: envelope.remote_slice_results_v15.clone(),
        cross_runtime_replay_tickets_v15: envelope.cross_runtime_replay_tickets_v15.clone(),
        dispute_bundles_v15: envelope.dispute_bundles_v15.clone(),
        disclosure_policies_v15: envelope.disclosure_policies_v15.clone(),
        disclosure_budgets_v15: envelope.disclosure_budgets_v15.clone(),
        records,
    })
}

fn derive_execution_context_v2(envelope: &ExportEnvelopeV2) -> ExecutionContextV1 {
    let mut ctx = ExecutionContextV1::new(
        envelope
            .trace_ctx
            .clone()
            .unwrap_or_else(TraceCtx::generate),
    );
    ctx.replay_link = envelope
        .evidence_bundle
        .as_ref()
        .and_then(|bundle| bundle.replay_handle.clone());
    ctx.attempt_id = envelope
        .evidence_bundle
        .as_ref()
        .and_then(|bundle| bundle.attempt_id.clone());
    ctx.trial_id = envelope
        .evidence_bundle
        .as_ref()
        .and_then(|bundle| bundle.trial_id.clone());
    ctx.workload_class = Some("forge_export".into());
    ctx.environment_fingerprint = envelope.export_meta.as_ref().and_then(|meta| {
        meta.comparability_snapshot_version
            .as_ref()
            .map(|value| format!("comparability_snapshot:{value}"))
    });
    ctx.provider_route = envelope
        .export_meta
        .as_ref()
        .map(|meta| meta.authority.as_str().into());
    if envelope.trace_ctx.is_none() {
        ctx.degradation_markers
            .push("missing_source_trace_ctx".into());
        ctx.dispatch_outcome = DispatchOutcomeV1::Degraded;
    }
    ctx
}

fn derive_execution_context_v3(envelope: &ExportEnvelopeV3) -> ExecutionContextV1 {
    derive_execution_context_v2(&ExportEnvelopeV2 {
        envelope_id: envelope.envelope_id.clone(),
        schema_version: EXPORT_ENVELOPE_V2_SCHEMA.into(),
        content_digest: envelope.content_digest.clone(),
        source_authority: envelope.source_authority.clone(),
        scope_key: envelope.scope_key.clone(),
        trace_ctx: envelope.trace_ctx.clone(),
        exported_at: envelope.exported_at.clone(),
        export_meta: envelope.export_meta.clone(),
        evidence_bundle: envelope.evidence_bundle.clone(),
        records: envelope
            .records
            .iter()
            .map(|record| record.record.clone())
            .collect(),
    })
}

fn derive_episode_bundle_v2(
    envelope: &ExportEnvelopeV2,
    execution_context: &ExecutionContextV1,
) -> Result<Option<EpisodeBundleV1>, BridgeError> {
    let Some(bundle) = envelope.evidence_bundle.as_ref() else {
        return Ok(None);
    };
    let episode_record = envelope
        .records
        .iter()
        .find_map(|record| match record {
            ExportRecord::Episode(episode) => Some(episode),
            _ => None,
        })
        .ok_or_else(|| BridgeError::InvalidRecord {
            reason: "canonical bundle-bearing export is missing an episode record".into(),
        })?;
    let episode_id =
        episode_record
            .episode_id
            .clone()
            .ok_or_else(|| BridgeError::InvalidRecord {
                reason: "canonical bundle-bearing export is missing episode_id".into(),
            })?;
    let claim_version_ids = envelope
        .records
        .iter()
        .filter_map(|record| match record {
            ExportRecord::Claim(claim) => claim.claim_version_id.as_ref().map(|id| id.to_string()),
            _ => None,
        })
        .collect();
    let relation_version_ids = envelope
        .records
        .iter()
        .filter_map(|record| match record {
            ExportRecord::Relation(relation) => relation
                .relation_version_id
                .as_ref()
                .map(|id| id.to_string()),
            _ => None,
        })
        .collect();
    let source_evidence_pointers = envelope
        .records
        .iter()
        .filter_map(|record| match record {
            ExportRecord::EvidenceRef(reference) => Some(reference.fetch_handle.clone()),
            _ => None,
        })
        .collect();
    let source_receipt_digests = bundle
        .raw_receipt_handle
        .as_ref()
        .map_or_else(Vec::new, |value| vec![value.clone()]);
    Ok(Some(EpisodeBundleV1 {
        schema_version: semantic_memory_forge::EPISODE_BUNDLE_V1_SCHEMA.into(),
        bundle_id: bundle.id.to_string(),
        episode_id,
        primary_document_id: episode_record.document_id.clone(),
        namespace: envelope.scope_key.namespace.clone(),
        scope_key: envelope.scope_key.clone(),
        valid_from: envelope.records.iter().find_map(|record| match record {
            ExportRecord::Claim(claim) => claim.valid_from.clone(),
            _ => None,
        }),
        valid_to: envelope.records.iter().find_map(|record| match record {
            ExportRecord::Claim(claim) => claim.valid_to.clone(),
            _ => None,
        }),
        exported_at: envelope.exported_at.clone(),
        recorded_at: None,
        source_envelope_id: envelope.envelope_id.clone(),
        content_digest: envelope.content_digest.clone(),
        source_evidence_pointers,
        source_receipt_digests,
        claim_version_ids,
        relation_version_ids,
        verification_summary: bundle.verification_summary.clone(),
        refutation_artifact_ids: bundle
            .refutation_artifacts
            .iter()
            .map(|artifact| artifact.artifact_id.clone())
            .collect(),
        control_plane_refs: envelope
            .export_meta
            .as_ref()
            .and_then(|meta| meta.run_id.clone())
            .map_or_else(Vec::new, |run_id| vec![format!("forge_run:{run_id}")]),
        execution_context: execution_context.clone(),
        thin_export: envelope.evidence_bundle.is_none(),
        supersedes_bundle_id: None,
        evidence_bundle_id: Some(bundle.id.to_string()),
    }))
}

fn derive_episode_bundle_v3(
    envelope: &ExportEnvelopeV3,
    execution_context: &ExecutionContextV1,
) -> Result<Option<EpisodeBundleV1>, BridgeError> {
    derive_episode_bundle_v2(
        &ExportEnvelopeV2 {
            envelope_id: envelope.envelope_id.clone(),
            schema_version: EXPORT_ENVELOPE_V2_SCHEMA.into(),
            content_digest: envelope.content_digest.clone(),
            source_authority: envelope.source_authority.clone(),
            scope_key: envelope.scope_key.clone(),
            trace_ctx: envelope.trace_ctx.clone(),
            exported_at: envelope.exported_at.clone(),
            export_meta: envelope.export_meta.clone(),
            evidence_bundle: envelope.evidence_bundle.clone(),
            records: envelope
                .records
                .iter()
                .map(|record| record.record.clone())
                .collect(),
        },
        execution_context,
    )
}

fn claim_projection_state(
    metadata: Option<&serde_json::Value>,
) -> (ClaimState, ProjectionFreshness, ContradictionStatus) {
    let Some(summary) = metadata.and_then(|metadata| metadata.get("verification_summary")) else {
        return (
            ClaimState::Active,
            ProjectionFreshness::Current,
            ContradictionStatus::None,
        );
    };

    let lifecycle_state = summary
        .get("lifecycle_state")
        .and_then(serde_json::Value::as_str);
    let notes = summary
        .get("notes")
        .and_then(serde_json::Value::as_array)
        .map(|notes| {
            notes
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()
                .join("; ")
        })
        .filter(|notes| !notes.is_empty());

    match lifecycle_state {
        Some("unverified") => (
            ClaimState::PendingReview,
            ProjectionFreshness::Current,
            ContradictionStatus::None,
        ),
        Some("verified") => (
            ClaimState::Active,
            ProjectionFreshness::Current,
            ContradictionStatus::None,
        ),
        Some("contradicted") => (
            ClaimState::Disputed,
            ProjectionFreshness::Current,
            ContradictionStatus::PossibleContradiction {
                description: notes.unwrap_or_else(|| {
                    "exported verification summary marked claim as contradicted".into()
                }),
            },
        ),
        Some("superseded") => (
            ClaimState::Superseded,
            ProjectionFreshness::Superseded,
            ContradictionStatus::None,
        ),
        _ => (
            ClaimState::Active,
            ProjectionFreshness::Current,
            ContradictionStatus::None,
        ),
    }
}

/// Transform a single export record into an import projection record.
fn transform_record(
    record: &ExportRecord,
    envelope: &ExportEnvelopeV1,
) -> Result<ImportProjectionRecord, BridgeError> {
    match record {
        ExportRecord::Claim(claim) => {
            let claim_id = claim.claim_id.clone().unwrap_or_else(ClaimId::generate);
            let claim_version_id = claim
                .claim_version_id
                .clone()
                .unwrap_or_else(ClaimVersionId::generate);
            let (claim_state, freshness, contradiction_status) =
                claim_projection_state(claim.metadata.as_ref());

            Ok(ImportProjectionRecord::ClaimVersion(ImportClaimVersion {
                claim_id,
                claim_version_id,
                claim_state,
                projection_family: claim.projection_family.clone(),
                subject_entity_id: claim.subject_entity_id.clone(),
                predicate: claim.predicate.clone(),
                object_anchor: claim.object_anchor.clone(),
                scope_key: envelope.scope_key.clone(),
                valid_from: claim.valid_from.clone(),
                valid_to: claim.valid_to.clone(),
                preferred_open: claim.valid_to.is_none(),
                source_envelope_id: envelope.envelope_id.clone(),
                source_authority: envelope.source_authority.clone(),
                trace_ctx: envelope.trace_ctx.clone(),
                freshness,
                contradiction_status,
                supersedes_claim_version_id: claim.supersedes_claim_version_id.clone(),
                content: claim.content.clone(),
                confidence: claim.confidence,
                metadata: claim.metadata.clone(),
            }))
        }

        ExportRecord::Relation(rel) => {
            let relation_version_id = rel
                .relation_version_id
                .clone()
                .unwrap_or_else(RelationVersionId::generate);

            Ok(ImportProjectionRecord::RelationVersion(
                ImportRelationVersion {
                    relation_version_id,
                    subject_entity_id: rel.subject_entity_id.clone(),
                    predicate: rel.predicate.clone(),
                    object_anchor: rel.object_anchor.clone(),
                    scope_key: envelope.scope_key.clone(),
                    claim_id: rel.source_claim_id.clone(),
                    source_episode_id: rel.source_episode_id.clone(),
                    valid_from: rel.valid_from.clone(),
                    valid_to: rel.valid_to.clone(),
                    preferred_open: rel.valid_to.is_none(),
                    supersedes_relation_version_id: rel.supersedes_relation_version_id.clone(),
                    contradiction_status: ContradictionStatus::None,
                    source_confidence: rel.confidence,
                    projection_family: rel.projection_family.clone(),
                    source_envelope_id: envelope.envelope_id.clone(),
                    source_authority: envelope.source_authority.clone(),
                    trace_ctx: envelope.trace_ctx.clone(),
                    freshness: ProjectionFreshness::Current,
                    metadata: rel.metadata.clone(),
                },
            ))
        }

        ExportRecord::Episode(ep) => {
            let episode_id =
                ep.episode_id
                    .clone()
                    .ok_or_else(|| BridgeError::MissingEpisodeIdentity {
                        record_context: format!(
                            "legacy import at {}",
                            ep.experiment_id.as_deref().unwrap_or("unknown")
                        ),
                    })?;

            Ok(ImportProjectionRecord::Episode(ImportEpisodeRecord {
                episode_id,
                document_id: ep.document_id.clone(),
                cause_ids: ep.cause_ids.clone(),
                effect_type: ep.effect_type.clone(),
                outcome: ep.outcome.clone(),
                confidence: ep.confidence,
                experiment_id: ep.experiment_id.clone(),
                source_envelope_id: envelope.envelope_id.clone(),
                source_authority: envelope.source_authority.clone(),
                trace_ctx: envelope.trace_ctx.clone(),
                metadata: ep.metadata.clone(),
            }))
        }

        ExportRecord::EntityAlias(alias) => {
            let scope = alias
                .scope
                .clone()
                .unwrap_or_else(|| envelope.scope_key.clone());

            Ok(ImportProjectionRecord::EntityAlias(ImportEntityAlias {
                canonical_entity_id: alias.canonical_entity_id.clone(),
                alias_text: alias.alias_text.clone(),
                alias_source: alias.alias_source.clone(),
                match_evidence: alias.match_evidence.clone(),
                confidence: alias.confidence,
                merge_decision: MergeDecision::PendingReview,
                scope,
                review_state: ReviewState::PendingReview,
                is_human_confirmed: false,
                is_human_confirmed_final: false,
                superseded_by_entity_id: alias.superseded_by_entity_id.clone(),
                split_from_entity_id: alias.split_from_entity_id.clone(),
                source_envelope_id: envelope.envelope_id.clone(),
            }))
        }

        ExportRecord::EvidenceRef(ev) => {
            Ok(ImportProjectionRecord::EvidenceRef(ImportEvidenceRef {
                claim_id: ev.claim_id.clone(),
                claim_version_id: ev.claim_version_id.clone(),
                fetch_handle: ev.fetch_handle.clone(),
                source_authority: ev.source_authority.clone(),
                source_envelope_id: envelope.envelope_id.clone(),
                metadata: ev.metadata.clone(),
            }))
        }
    }
}

fn transform_record_v3(
    record: &ExportRecordV3,
    envelope: &ExportEnvelopeV3,
) -> Result<ImportProjectionRecordV3, BridgeError> {
    let record_only_envelope = ExportEnvelopeV2 {
        envelope_id: envelope.envelope_id.clone(),
        schema_version: EXPORT_ENVELOPE_V2_SCHEMA.into(),
        content_digest: envelope.content_digest.clone(),
        source_authority: envelope.source_authority.clone(),
        scope_key: envelope.scope_key.clone(),
        trace_ctx: envelope.trace_ctx.clone(),
        exported_at: envelope.exported_at.clone(),
        export_meta: envelope.export_meta.clone(),
        evidence_bundle: envelope.evidence_bundle.clone(),
        records: vec![record.record.clone()],
    };

    let import_record = transform_record_v2(&record.record, &record_only_envelope)?;
    Ok(ImportProjectionRecordV3 {
        record: import_record,
        semantics: record.semantics.clone(),
    })
}

fn transform_record_v2(
    record: &ExportRecord,
    envelope: &ExportEnvelopeV2,
) -> Result<ImportProjectionRecord, BridgeError> {
    match record {
        ExportRecord::Claim(claim) => {
            let claim_id = claim.claim_id.clone().unwrap_or_else(ClaimId::generate);
            let claim_version_id = claim
                .claim_version_id
                .clone()
                .unwrap_or_else(ClaimVersionId::generate);
            let (claim_state, freshness, contradiction_status) =
                claim_projection_state(claim.metadata.as_ref());

            Ok(ImportProjectionRecord::ClaimVersion(ImportClaimVersion {
                claim_id,
                claim_version_id,
                claim_state,
                projection_family: claim.projection_family.clone(),
                subject_entity_id: claim.subject_entity_id.clone(),
                predicate: claim.predicate.clone(),
                object_anchor: claim.object_anchor.clone(),
                scope_key: envelope.scope_key.clone(),
                valid_from: claim.valid_from.clone(),
                valid_to: claim.valid_to.clone(),
                preferred_open: claim.valid_to.is_none(),
                source_envelope_id: envelope.envelope_id.clone(),
                source_authority: envelope.source_authority.clone(),
                trace_ctx: envelope.trace_ctx.clone(),
                freshness,
                contradiction_status,
                supersedes_claim_version_id: claim.supersedes_claim_version_id.clone(),
                content: claim.content.clone(),
                confidence: claim.confidence,
                metadata: claim.metadata.clone(),
            }))
        }
        ExportRecord::Relation(rel) => {
            let relation_version_id = rel
                .relation_version_id
                .clone()
                .unwrap_or_else(RelationVersionId::generate);

            Ok(ImportProjectionRecord::RelationVersion(
                ImportRelationVersion {
                    relation_version_id,
                    subject_entity_id: rel.subject_entity_id.clone(),
                    predicate: rel.predicate.clone(),
                    object_anchor: rel.object_anchor.clone(),
                    scope_key: envelope.scope_key.clone(),
                    claim_id: rel.source_claim_id.clone(),
                    source_episode_id: rel.source_episode_id.clone(),
                    valid_from: rel.valid_from.clone(),
                    valid_to: rel.valid_to.clone(),
                    preferred_open: rel.valid_to.is_none(),
                    supersedes_relation_version_id: rel.supersedes_relation_version_id.clone(),
                    contradiction_status: ContradictionStatus::None,
                    source_confidence: rel.confidence,
                    projection_family: rel.projection_family.clone(),
                    source_envelope_id: envelope.envelope_id.clone(),
                    source_authority: envelope.source_authority.clone(),
                    trace_ctx: envelope.trace_ctx.clone(),
                    freshness: ProjectionFreshness::Current,
                    metadata: rel.metadata.clone(),
                },
            ))
        }
        ExportRecord::Episode(ep) => {
            let episode_id =
                ep.episode_id
                    .clone()
                    .ok_or_else(|| BridgeError::MissingEpisodeIdentity {
                        record_context: format!(
                            "legacy import at {}",
                            ep.experiment_id.as_deref().unwrap_or("unknown")
                        ),
                    })?;
            Ok(ImportProjectionRecord::Episode(ImportEpisodeRecord {
                episode_id,
                document_id: ep.document_id.clone(),
                cause_ids: ep.cause_ids.clone(),
                effect_type: ep.effect_type.clone(),
                outcome: ep.outcome.clone(),
                confidence: ep.confidence,
                experiment_id: ep.experiment_id.clone(),
                source_envelope_id: envelope.envelope_id.clone(),
                source_authority: envelope.source_authority.clone(),
                trace_ctx: envelope.trace_ctx.clone(),
                metadata: ep.metadata.clone(),
            }))
        }
        ExportRecord::EntityAlias(alias) => {
            let scope = alias
                .scope
                .clone()
                .unwrap_or_else(|| envelope.scope_key.clone());
            Ok(ImportProjectionRecord::EntityAlias(ImportEntityAlias {
                canonical_entity_id: alias.canonical_entity_id.clone(),
                alias_text: alias.alias_text.clone(),
                alias_source: alias.alias_source.clone(),
                match_evidence: alias.match_evidence.clone(),
                confidence: alias.confidence,
                merge_decision: MergeDecision::PendingReview,
                scope,
                review_state: ReviewState::PendingReview,
                is_human_confirmed: false,
                is_human_confirmed_final: false,
                superseded_by_entity_id: alias.superseded_by_entity_id.clone(),
                split_from_entity_id: alias.split_from_entity_id.clone(),
                source_envelope_id: envelope.envelope_id.clone(),
            }))
        }
        ExportRecord::EvidenceRef(ev) => {
            Ok(ImportProjectionRecord::EvidenceRef(ImportEvidenceRef {
                claim_id: ev.claim_id.clone(),
                claim_version_id: ev.claim_version_id.clone(),
                fetch_handle: ev.fetch_handle.clone(),
                source_authority: ev.source_authority.clone(),
                source_envelope_id: envelope.envelope_id.clone(),
                metadata: ev.metadata.clone(),
            }))
        }
    }
}

/// Validate that an export envelope's version is compatible with this bridge.
pub fn is_compatible_version(schema_version: &str) -> bool {
    matches!(
        schema_version,
        EXPORT_ENVELOPE_V1_SCHEMA | EXPORT_ENVELOPE_V2_SCHEMA | EXPORT_ENVELOPE_V3_SCHEMA
    )
}

/// Create a trace context for the bridge transformation, chaining from
/// the source envelope's trace context if available.
pub fn bridge_trace_ctx(source: Option<&TraceCtx>) -> TraceCtx {
    match source {
        Some(parent) => {
            let span_id = &uuid::Uuid::new_v4().as_simple().to_string()[..16];
            parent.child(span_id)
        }
        None => TraceCtx::generate(),
    }
}

#[cfg(test)]
#[path = "transform_tests.rs"]
mod tests;
