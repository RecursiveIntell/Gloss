#![allow(deprecated)]

//! Forge-owned export envelope schema and metadata.
//!
//! Forge owns raw verification truth. Export envelopes are Forge-owned artifacts
//! that flow through the bridge into memory.
//!
//! V3 is the canonical export contract. Within V3:
//! - grouping semantics (`claim_family_id`, `assertion_group_id`,
//!   `relation_group_id`) are canonical when the source can supply them,
//! - `constraint_seed_kind` is canonical for relation/episode-style kernel
//!   seeds but may remain absent on thin claim exports,
//! - lineage such as `supersedes_*` and `derivation_seed_ids` must only be
//!   emitted when the source bundle actually knows them,
//! - absent semantics are an explicit thin-export state, not permission to
//!   invent missing truth downstream.

use crate::bundle::EvidenceBundle;
use crate::{ClaimStateV13, ContradictionWitnessV1, RetractionRecordV1, SupportSetV1};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use stack_ids::{
    AssertionGroupId, ClaimFamilyId, ClaimId, ClaimVersionId, ConstraintGroupId, ContentDigest,
    ContradictionGroupId, DigestBuilder, EntityId, EnvelopeId, EpisodeId, JointEvidenceGroupId,
    RelationGroupId, RelationVersionId, ScopeKey, TraceCtx,
};
use std::any::type_name;
use thiserror::Error;
use tracing::warn;

/// Authority marker for Forge-produced exports.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportAuthority {
    /// Standard Forge verification pipeline.
    Forge,
    /// External authority (e.g., manual import, third-party tool).
    External { name: String },
}

impl ExportAuthority {
    /// Returns the stable authority label carried into exported envelopes.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Forge => "forge",
            Self::External { name } => name,
        }
    }
}

/// Metadata about a Forge export operation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ForgeExportMeta {
    /// Authority that produced the export.
    pub authority: ExportAuthority,
    /// Verification run identifier.
    pub run_id: Option<String>,
    /// Whether direct-write was used (exceptional, auditable, disabled by default).
    pub direct_write: bool,
    /// Comparability snapshot version at time of export.
    pub comparability_snapshot_version: Option<String>,
    /// When the export was produced.
    pub exported_at: String,
}

/// Schema version for the legacy compatibility export envelope.
///
/// Phase status: migration-only
/// Removal condition: remove when all consumers have migrated to `ExportEnvelopeV3`
#[deprecated(
    since = "0.2.0",
    note = "ExportEnvelopeV1 is compatibility-only. Use ExportEnvelopeV3 as the canonical export contract."
)]
pub const EXPORT_ENVELOPE_V1_SCHEMA: &str = "export_envelope_v1";
/// Schema version for ExportEnvelopeV2.
pub const EXPORT_ENVELOPE_V2_SCHEMA: &str = "export_envelope_v2";
/// Schema version for the richer kernel-ready export envelope.
pub const EXPORT_ENVELOPE_V3_SCHEMA: &str = "export_envelope_v3";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintSeedKind {
    Hyperedge,
    MutualExclusion,
    TemporalCoherence,
    CausalRefutation,
    NuisanceDisclosure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CausalRoleHint {
    Treatment,
    Outcome,
    Confounder,
    Instrument,
    EffectModifier,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionVisibilityClass {
    #[default]
    Standard,
    Restricted,
    AuditOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExportConfidenceClass {
    Verified,
    Reviewed,
    Heuristic,
    #[default]
    ThinExport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NuisanceSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency_set_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scope_mismatch_markers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measurement_notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selection_bias_markers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub struct ExportRecordSemanticsV3 {
    /// Canonical claim lineage group when the exporter knows the claim family.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_family_id: Option<ClaimFamilyId>,
    /// Canonical assertion/hyperedge grouping for claim-version style records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assertion_group_id: Option<AssertionGroupId>,
    /// Canonical relation grouping for relation-version style records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation_group_id: Option<RelationGroupId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joint_evidence_group_id: Option<JointEvidenceGroupId>,
    /// Optional because thin claim exports may group via assertion/family IDs
    /// without emitting an explicit kernel seed kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraint_seed_kind: Option<ConstraintSeedKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub treatment_hint: Option<CausalRoleHint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome_hint: Option<CausalRoleHint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confounder_hint: Option<CausalRoleHint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instrument_hint: Option<CausalRoleHint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_modifier_hint: Option<CausalRoleHint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contradiction_candidate_group_id: Option<ContradictionGroupId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutual_exclusion_group_id: Option<ConstraintGroupId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparability_snapshot_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nuisance_snapshot: Option<NuisanceSnapshot>,
    pub projection_visibility_class: ProjectionVisibilityClass,
    pub export_confidence_class: ExportConfidenceClass,
    /// Exporter-supplied lineage only. Bridge/runtime/kernel must not invent it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derivation_seed_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_priority_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExportRecordV3 {
    pub record: ExportRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantics: Option<ExportRecordSemanticsV3>,
}

/// Errors produced by Forge export schema validation.
#[derive(Debug, Error)]
pub enum ExportEnvelopeError {
    /// The export envelope is structurally invalid.
    #[error("invalid envelope: {reason}")]
    InvalidEnvelope { reason: String },

    /// Schema version mismatch.
    #[error("incompatible version: expected {expected}, got {actual}")]
    IncompatibleVersion { expected: String, actual: String },

    /// Content digest does not match computed value.
    #[error("digest mismatch: expected {expected}, got {actual}")]
    DigestMismatch { expected: String, actual: String },

    /// Failed to compute content digest.
    #[error("digest computation failed: {reason}")]
    DigestComputationFailed { reason: String },
}

impl ExportEnvelopeError {
    /// Returns a stable string discriminant for this envelope error kind.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidEnvelope { .. } => "invalid_envelope",
            Self::IncompatibleVersion { .. } => "incompatible_version",
            Self::DigestMismatch { .. } => "digest_mismatch",
            Self::DigestComputationFailed { .. } => "digest_computation_failed",
        }
    }
}

/// Forge-owned export envelope containing projection data.
///
/// The content digest covers: `source_authority`, `scope_key`, and all
/// `records` (serialized with canonical JSON). The `envelope_id`,
/// `exported_at`, and `trace_ctx` are excluded from the digest because
/// they are metadata about the export, not the projection content.
///
/// Phase status: migration-only
/// Removal condition: remove when all consumers have migrated to `ExportEnvelopeV3`
#[deprecated(
    since = "0.2.0",
    note = "ExportEnvelopeV1 is compatibility-only. Use ExportEnvelopeV3 as the canonical export contract."
)]
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExportEnvelopeV1 {
    /// Unique envelope identity, assigned by Forge.
    pub envelope_id: EnvelopeId,
    /// Always `"export_envelope_v1"` for this version.
    pub schema_version: String,
    /// BLAKE3 content digest for idempotent deduplication.
    pub content_digest: ContentDigest,
    /// Forge or other producing authority.
    pub source_authority: String,
    /// Target scope for all records in this envelope.
    pub scope_key: ScopeKey,
    /// Cross-crate trace context.
    pub trace_ctx: Option<TraceCtx>,
    /// ISO 8601 timestamp of when this envelope was exported.
    pub exported_at: String,
    /// The export records.
    pub records: Vec<ExportRecord>,
}

impl ExportEnvelopeV1 {
    /// Validate envelope structure.
    pub fn validate(&self) -> Result<(), ExportEnvelopeError> {
        validate_envelope_fields(
            &self.envelope_id,
            &self.schema_version,
            EXPORT_ENVELOPE_V1_SCHEMA,
            &self.source_authority,
            &self.scope_key,
            &self.records,
        )?;

        let computed =
            Self::compute_digest(&self.source_authority, &self.scope_key, &self.records)?;
        if computed != self.content_digest {
            return Err(ExportEnvelopeError::DigestMismatch {
                expected: self.content_digest.hex().to_string(),
                actual: computed.hex().to_string(),
            });
        }

        Ok(())
    }

    /// Compute the canonical content digest for this envelope's content.
    pub fn compute_digest(
        source_authority: &str,
        scope_key: &ScopeKey,
        records: &[ExportRecord],
    ) -> Result<ContentDigest, ExportEnvelopeError> {
        compute_digest_inner(source_authority, scope_key, records, None, None)
    }
}

/// Forge-owned export envelope containing projection data plus the canonical
/// evidence bundle that produced those projections.
///
/// The bridge remains deterministic and non-semantic: it may transform the
/// projection records and carry the source schema version forward, but it does
/// not reinterpret or mutate the embedded evidence bundle. For digest
/// stability, envelope/export timestamps, `ForgeExportMeta::exported_at`, and
/// the embedded bundle's `created_at` timestamp are treated as metadata rather
/// than projection content.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExportEnvelopeV2 {
    /// Unique envelope identity, assigned by Forge.
    pub envelope_id: EnvelopeId,
    /// Always `"export_envelope_v2"` for this version.
    pub schema_version: String,
    /// BLAKE3 content digest for idempotent deduplication.
    pub content_digest: ContentDigest,
    /// Forge or other producing authority.
    pub source_authority: String,
    /// Target scope for all records in this envelope.
    pub scope_key: ScopeKey,
    /// Cross-crate trace context.
    pub trace_ctx: Option<TraceCtx>,
    /// ISO 8601 timestamp of when this envelope was exported.
    pub exported_at: String,
    /// Optional export metadata retained as first-class provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_meta: Option<ForgeExportMeta>,
    /// Canonical Forge evidence substrate backing the projected records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_bundle: Option<EvidenceBundle>,
    /// The export records.
    pub records: Vec<ExportRecord>,
}

/// CLIB-013: Canonical Forge export envelope (v3).
///
/// ## Field categories
///
/// **Core fields** (always populated):
/// `envelope_id`, `schema_version`, `content_digest`, `source_authority`,
/// `scope_key`, `exported_at`, `records`.
///
/// **Active v13 fields** (populated by Forge when support-algebra artifacts exist):
/// `support_sets`, `contradiction_witnesses`, `retraction_records`, `claim_states_v13`.
///
/// **v14 extension points** (schema-reserved for mechanism-runtime / experiment lanes;
/// populated only when the owning lane has produced matching artifacts):
/// `intervention_bundles_v14` through `rollback_decisions_v14`.
///
/// **v15 extension points** (schema-reserved for attestation-exchange / admission lanes;
/// populated only when the owning lane has produced matching artifacts):
/// `attestation_envelopes_v15` through `disclosure_budgets_v15`.
///
/// Extension fields use `Vec<Value>` + `skip_serializing_if = "Vec::is_empty"` so they
/// carry zero overhead in serialized envelopes when unused.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExportEnvelopeV3 {
    /// Unique envelope identity, assigned by Forge.
    pub envelope_id: EnvelopeId,
    /// Always `"export_envelope_v3"` for this version.
    pub schema_version: String,
    /// BLAKE3 content digest for idempotent deduplication.
    pub content_digest: ContentDigest,
    /// Forge or other producing authority.
    pub source_authority: String,
    /// Target scope for all records in this envelope.
    pub scope_key: ScopeKey,
    /// Cross-crate trace context.
    pub trace_ctx: Option<TraceCtx>,
    /// ISO 8601 timestamp of when this envelope was exported.
    pub exported_at: String,
    /// Optional export metadata retained as first-class provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_meta: Option<ForgeExportMeta>,
    /// Canonical Forge evidence substrate backing the projected records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_bundle: Option<EvidenceBundle>,
    /// Additive v13 support algebra artifacts owned by Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub support_sets: Vec<SupportSetV1>,
    /// Additive v13 contradiction witnesses owned by Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contradiction_witnesses: Vec<ContradictionWitnessV1>,
    /// Additive v13 retraction lineage artifacts owned by Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retraction_records: Vec<RetractionRecordV1>,
    /// Additive v13 claim-state artifacts owned by Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claim_states_v13: Vec<ClaimStateV13>,
    /// Additive v14 intervention artifacts preserved verbatim from the owner lane.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intervention_bundles_v14: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outcome_schemas_v14: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cohort_contracts_v14: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub counterfactual_slices_v14: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub experiment_cases_v14: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comparability_matrices_v14: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decision_traces_v14: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refuter_suites_v14: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refuter_results_v14: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub experiment_budgets_v14: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rollout_decisions_v14: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rollback_decisions_v14: Vec<Value>,
    /// Additive v15 attestation and admission artifacts preserved verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attestation_envelopes_v15: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trust_root_sets_v15: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_admission_policies_v15: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transparency_receipts_v15: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attestation_revocations_v15: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attestation_supersessions_v15: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remote_oracle_leases_v15: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remote_slice_requests_v15: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remote_slice_results_v15: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cross_runtime_replay_tickets_v15: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dispute_bundles_v15: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disclosure_policies_v15: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disclosure_budgets_v15: Vec<Value>,
    /// Rich projection records carrying kernel-relevant export semantics.
    pub records: Vec<ExportRecordV3>,
}

impl ExportEnvelopeV2 {
    /// Validate envelope structure.
    pub fn validate(&self) -> Result<(), ExportEnvelopeError> {
        validate_envelope_fields(
            &self.envelope_id,
            &self.schema_version,
            EXPORT_ENVELOPE_V2_SCHEMA,
            &self.source_authority,
            &self.scope_key,
            &self.records,
        )?;

        let computed = Self::compute_digest(
            &self.source_authority,
            &self.scope_key,
            &self.records,
            self.export_meta.as_ref(),
            self.evidence_bundle.as_ref(),
        )?;
        if computed != self.content_digest {
            return Err(ExportEnvelopeError::DigestMismatch {
                expected: self.content_digest.hex().to_string(),
                actual: computed.hex().to_string(),
            });
        }

        Ok(())
    }

    /// Compute the canonical content digest for this envelope's content.
    pub fn compute_digest(
        source_authority: &str,
        scope_key: &ScopeKey,
        records: &[ExportRecord],
        export_meta: Option<&ForgeExportMeta>,
        evidence_bundle: Option<&EvidenceBundle>,
    ) -> Result<ContentDigest, ExportEnvelopeError> {
        compute_digest_inner(
            source_authority,
            scope_key,
            records,
            export_meta,
            evidence_bundle,
        )
    }
}

impl ExportEnvelopeV3 {
    /// Validates the canonical v3 export envelope and its computed digest.
    pub fn validate(&self) -> Result<(), ExportEnvelopeError> {
        validate_envelope_fields_v3(
            &self.envelope_id,
            &self.schema_version,
            EXPORT_ENVELOPE_V3_SCHEMA,
            &self.source_authority,
            &self.scope_key,
            &self.records,
        )?;

        let computed = Self::compute_digest(
            &self.source_authority,
            &self.scope_key,
            &self.records,
            self.export_meta.as_ref(),
            self.evidence_bundle.as_ref(),
        )?;
        let computed = Self::compute_digest_with_v13(
            computed,
            &self.support_sets,
            &self.contradiction_witnesses,
            &self.retraction_records,
            &self.claim_states_v13,
        )?;
        let computed = Self::compute_digest_with_endgame(
            computed,
            &self.intervention_bundles_v14,
            &self.outcome_schemas_v14,
            &self.cohort_contracts_v14,
            &self.counterfactual_slices_v14,
            &self.experiment_cases_v14,
            &self.comparability_matrices_v14,
            &self.decision_traces_v14,
            &self.refuter_suites_v14,
            &self.refuter_results_v14,
            &self.experiment_budgets_v14,
            &self.rollout_decisions_v14,
            &self.rollback_decisions_v14,
            &self.attestation_envelopes_v15,
            &self.trust_root_sets_v15,
            &self.artifact_admission_policies_v15,
            &self.transparency_receipts_v15,
            &self.attestation_revocations_v15,
            &self.attestation_supersessions_v15,
            &self.remote_oracle_leases_v15,
            &self.remote_slice_requests_v15,
            &self.remote_slice_results_v15,
            &self.cross_runtime_replay_tickets_v15,
            &self.dispute_bundles_v15,
            &self.disclosure_policies_v15,
            &self.disclosure_budgets_v15,
        )?;
        if computed != self.content_digest {
            return Err(ExportEnvelopeError::DigestMismatch {
                expected: self.content_digest.hex().to_string(),
                actual: computed.hex().to_string(),
            });
        }

        Ok(())
    }

    /// Computes the canonical digest for the v3 envelope content before additive v13/v15 groups.
    pub fn compute_digest(
        source_authority: &str,
        scope_key: &ScopeKey,
        records: &[ExportRecordV3],
        export_meta: Option<&ForgeExportMeta>,
        evidence_bundle: Option<&EvidenceBundle>,
    ) -> Result<ContentDigest, ExportEnvelopeError> {
        compute_digest_inner(
            source_authority,
            scope_key,
            records,
            export_meta,
            evidence_bundle,
        )
    }

    /// Extends a base digest with additive v13 support-algebra artifacts.
    pub fn compute_digest_with_v13(
        base_digest: ContentDigest,
        support_sets: &[SupportSetV1],
        contradiction_witnesses: &[ContradictionWitnessV1],
        retraction_records: &[RetractionRecordV1],
        claim_states_v13: &[ClaimStateV13],
    ) -> Result<ContentDigest, ExportEnvelopeError> {
        if support_sets.is_empty()
            && contradiction_witnesses.is_empty()
            && retraction_records.is_empty()
            && claim_states_v13.is_empty()
        {
            return Ok(base_digest);
        }

        let mut builder = DigestBuilder::new();
        builder
            .update_json(&base_digest)
            .map_err(digest_computation_failed)?;
        if !support_sets.is_empty() {
            builder.separator();
            builder
                .update_json(support_sets)
                .map_err(digest_computation_failed)?;
        }
        if !contradiction_witnesses.is_empty() {
            builder.separator();
            builder
                .update_json(contradiction_witnesses)
                .map_err(digest_computation_failed)?;
        }
        if !retraction_records.is_empty() {
            builder.separator();
            builder
                .update_json(retraction_records)
                .map_err(digest_computation_failed)?;
        }
        if !claim_states_v13.is_empty() {
            builder.separator();
            builder
                .update_json(claim_states_v13)
                .map_err(digest_computation_failed)?;
        }
        Ok(builder.finalize())
    }

    #[allow(clippy::too_many_arguments)]
    /// Extends a base digest with additive v14/v15 endgame artifact groups.
    pub fn compute_digest_with_endgame(
        base_digest: ContentDigest,
        intervention_bundles_v14: &[Value],
        outcome_schemas_v14: &[Value],
        cohort_contracts_v14: &[Value],
        counterfactual_slices_v14: &[Value],
        experiment_cases_v14: &[Value],
        comparability_matrices_v14: &[Value],
        decision_traces_v14: &[Value],
        refuter_suites_v14: &[Value],
        refuter_results_v14: &[Value],
        experiment_budgets_v14: &[Value],
        rollout_decisions_v14: &[Value],
        rollback_decisions_v14: &[Value],
        attestation_envelopes_v15: &[Value],
        trust_root_sets_v15: &[Value],
        artifact_admission_policies_v15: &[Value],
        transparency_receipts_v15: &[Value],
        attestation_revocations_v15: &[Value],
        attestation_supersessions_v15: &[Value],
        remote_oracle_leases_v15: &[Value],
        remote_slice_requests_v15: &[Value],
        remote_slice_results_v15: &[Value],
        cross_runtime_replay_tickets_v15: &[Value],
        dispute_bundles_v15: &[Value],
        disclosure_policies_v15: &[Value],
        disclosure_budgets_v15: &[Value],
    ) -> Result<ContentDigest, ExportEnvelopeError> {
        let groups: [Option<serde_json::Value>; 25] = [
            (!intervention_bundles_v14.is_empty())
                .then(|| serde_json::json!(intervention_bundles_v14)),
            (!outcome_schemas_v14.is_empty()).then(|| serde_json::json!(outcome_schemas_v14)),
            (!cohort_contracts_v14.is_empty()).then(|| serde_json::json!(cohort_contracts_v14)),
            (!counterfactual_slices_v14.is_empty())
                .then(|| serde_json::json!(counterfactual_slices_v14)),
            (!experiment_cases_v14.is_empty()).then(|| serde_json::json!(experiment_cases_v14)),
            (!comparability_matrices_v14.is_empty())
                .then(|| serde_json::json!(comparability_matrices_v14)),
            (!decision_traces_v14.is_empty()).then(|| serde_json::json!(decision_traces_v14)),
            (!refuter_suites_v14.is_empty()).then(|| serde_json::json!(refuter_suites_v14)),
            (!refuter_results_v14.is_empty()).then(|| serde_json::json!(refuter_results_v14)),
            (!experiment_budgets_v14.is_empty()).then(|| serde_json::json!(experiment_budgets_v14)),
            (!rollout_decisions_v14.is_empty()).then(|| serde_json::json!(rollout_decisions_v14)),
            (!rollback_decisions_v14.is_empty()).then(|| serde_json::json!(rollback_decisions_v14)),
            (!attestation_envelopes_v15.is_empty())
                .then(|| serde_json::json!(attestation_envelopes_v15)),
            (!trust_root_sets_v15.is_empty()).then(|| serde_json::json!(trust_root_sets_v15)),
            (!artifact_admission_policies_v15.is_empty())
                .then(|| serde_json::json!(artifact_admission_policies_v15)),
            (!transparency_receipts_v15.is_empty())
                .then(|| serde_json::json!(transparency_receipts_v15)),
            (!attestation_revocations_v15.is_empty())
                .then(|| serde_json::json!(attestation_revocations_v15)),
            (!attestation_supersessions_v15.is_empty())
                .then(|| serde_json::json!(attestation_supersessions_v15)),
            (!remote_oracle_leases_v15.is_empty())
                .then(|| serde_json::json!(remote_oracle_leases_v15)),
            (!remote_slice_requests_v15.is_empty())
                .then(|| serde_json::json!(remote_slice_requests_v15)),
            (!remote_slice_results_v15.is_empty())
                .then(|| serde_json::json!(remote_slice_results_v15)),
            (!cross_runtime_replay_tickets_v15.is_empty())
                .then(|| serde_json::json!(cross_runtime_replay_tickets_v15)),
            (!dispute_bundles_v15.is_empty()).then(|| serde_json::json!(dispute_bundles_v15)),
            (!disclosure_policies_v15.is_empty())
                .then(|| serde_json::json!(disclosure_policies_v15)),
            (!disclosure_budgets_v15.is_empty()).then(|| serde_json::json!(disclosure_budgets_v15)),
        ];

        if groups.iter().all(Option::is_none) {
            return Ok(base_digest);
        }

        let mut builder = DigestBuilder::new();
        builder
            .update_json(&base_digest)
            .map_err(digest_computation_failed)?;
        for group in groups.into_iter().flatten() {
            builder.separator();
            builder
                .update_json(&group)
                .map_err(digest_computation_failed)?;
        }
        Ok(builder.finalize())
    }
}

impl ExportEnvelopeV2 {
    /// Enrich a V2 envelope into a V3 envelope by lifting any already-exported
    /// metadata into typed kernel semantics. Missing fields remain absent.
    pub fn enrich_to_v3(&self) -> Result<ExportEnvelopeV3, ExportEnvelopeError> {
        self.validate()?;

        let records = self
            .records
            .iter()
            .cloned()
            .map(|record| ExportRecordV3::enrich(record, self.export_meta.as_ref()))
            .collect::<Vec<_>>();

        Ok(ExportEnvelopeV3 {
            envelope_id: self.envelope_id.clone(),
            schema_version: EXPORT_ENVELOPE_V3_SCHEMA.into(),
            content_digest: ExportEnvelopeV3::compute_digest(
                &self.source_authority,
                &self.scope_key,
                &records,
                self.export_meta.as_ref(),
                self.evidence_bundle.as_ref(),
            )?,
            source_authority: self.source_authority.clone(),
            scope_key: self.scope_key.clone(),
            trace_ctx: self.trace_ctx.clone(),
            exported_at: self.exported_at.clone(),
            export_meta: self.export_meta.clone(),
            evidence_bundle: self.evidence_bundle.clone(),
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
            records,
        })
    }
}

impl ExportRecordV3 {
    /// Enriches a legacy export record with any typed v3 semantics the source already carried.
    pub fn enrich(record: ExportRecord, export_meta: Option<&ForgeExportMeta>) -> Self {
        let semantics = ExportRecordSemanticsV3::from_record(&record, export_meta);
        Self { record, semantics }
    }
}

impl ExportRecordSemanticsV3 {
    /// Extracts typed v3 semantics from the source record metadata when present.
    pub fn from_record(
        record: &ExportRecord,
        export_meta: Option<&ForgeExportMeta>,
    ) -> Option<Self> {
        let metadata = match record {
            ExportRecord::Claim(claim) => claim.metadata.as_ref(),
            ExportRecord::Relation(relation) => relation.metadata.as_ref(),
            ExportRecord::Episode(episode) => episode.metadata.as_ref(),
            ExportRecord::EntityAlias(alias) => alias.match_evidence.as_ref(),
            ExportRecord::EvidenceRef(evidence) => evidence.metadata.as_ref(),
        };

        let semantics_root = metadata
            .and_then(|metadata| metadata.get("kernel_semantics_v3"))
            .or(metadata);

        let claim_family_id = semantics_root
            .and_then(|root| json_string(root, "claim_family_id"))
            .map(ClaimFamilyId::new);
        let assertion_group_id = semantics_root
            .and_then(|root| json_string(root, "assertion_group_id"))
            .map(AssertionGroupId::new);
        let relation_group_id = semantics_root
            .and_then(|root| json_string(root, "relation_group_id"))
            .map(RelationGroupId::new);
        let joint_evidence_group_id = semantics_root
            .and_then(|root| json_string(root, "joint_evidence_group_id"))
            .map(JointEvidenceGroupId::new);
        let contradiction_candidate_group_id = semantics_root
            .and_then(|root| json_string(root, "contradiction_candidate_group_id"))
            .map(ContradictionGroupId::new);
        let mutual_exclusion_group_id = semantics_root
            .and_then(|root| json_string(root, "mutual_exclusion_group_id"))
            .map(ConstraintGroupId::new);
        let constraint_seed_kind =
            semantics_root.and_then(|root| json_enum(root, "constraint_seed_kind"));
        let treatment_hint = semantics_root.and_then(|root| json_enum(root, "treatment_hint"));
        let outcome_hint = semantics_root.and_then(|root| json_enum(root, "outcome_hint"));
        let confounder_hint = semantics_root.and_then(|root| json_enum(root, "confounder_hint"));
        let instrument_hint = semantics_root.and_then(|root| json_enum(root, "instrument_hint"));
        let effect_modifier_hint =
            semantics_root.and_then(|root| json_enum(root, "effect_modifier_hint"));
        let nuisance_snapshot =
            semantics_root.and_then(|root| json_object_enum(root, "nuisance_snapshot"));
        let projection_visibility_class = semantics_root
            .and_then(|root| json_enum(root, "projection_visibility_class"))
            .unwrap_or_default();
        let export_confidence_class = semantics_root
            .and_then(|root| json_enum(root, "export_confidence_class"))
            .unwrap_or_else(|| {
                if semantics_root.is_some() || export_meta.is_some() {
                    ExportConfidenceClass::Reviewed
                } else {
                    ExportConfidenceClass::ThinExport
                }
            });
        let comparability_snapshot_version = semantics_root
            .and_then(|root| json_string(root, "comparability_snapshot_version"))
            .or_else(|| export_meta.and_then(|meta| meta.comparability_snapshot_version.clone()));
        let derivation_seed_ids = semantics_root
            .and_then(|root| root.get("derivation_seed_ids"))
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let review_priority_hint =
            semantics_root.and_then(|root| json_string(root, "review_priority_hint"));

        let has_any = claim_family_id.is_some()
            || assertion_group_id.is_some()
            || relation_group_id.is_some()
            || joint_evidence_group_id.is_some()
            || constraint_seed_kind.is_some()
            || treatment_hint.is_some()
            || outcome_hint.is_some()
            || confounder_hint.is_some()
            || instrument_hint.is_some()
            || effect_modifier_hint.is_some()
            || contradiction_candidate_group_id.is_some()
            || mutual_exclusion_group_id.is_some()
            || comparability_snapshot_version.is_some()
            || nuisance_snapshot.is_some()
            || !derivation_seed_ids.is_empty()
            || review_priority_hint.is_some()
            || semantics_root.is_some();

        has_any.then_some(Self {
            claim_family_id,
            assertion_group_id,
            relation_group_id,
            joint_evidence_group_id,
            constraint_seed_kind,
            treatment_hint,
            outcome_hint,
            confounder_hint,
            instrument_hint,
            effect_modifier_hint,
            contradiction_candidate_group_id,
            mutual_exclusion_group_id,
            comparability_snapshot_version,
            nuisance_snapshot,
            projection_visibility_class,
            export_confidence_class,
            derivation_seed_ids,
            review_priority_hint,
        })
    }
}

impl From<ExportEnvelopeV1> for ExportEnvelopeV2 {
    fn from(value: ExportEnvelopeV1) -> Self {
        Self {
            envelope_id: value.envelope_id,
            schema_version: EXPORT_ENVELOPE_V2_SCHEMA.into(),
            content_digest: value.content_digest,
            source_authority: value.source_authority,
            scope_key: value.scope_key,
            trace_ctx: value.trace_ctx,
            exported_at: value.exported_at,
            export_meta: None,
            evidence_bundle: None,
            records: value.records,
        }
    }
}

impl std::convert::TryFrom<ExportEnvelopeV2> for ExportEnvelopeV1 {
    type Error = ExportEnvelopeError;

    fn try_from(value: ExportEnvelopeV2) -> Result<Self, Self::Error> {
        value.validate()?;
        let content_digest = ExportEnvelopeV1::compute_digest(
            &value.source_authority,
            &value.scope_key,
            &value.records,
        )?;
        Ok(Self {
            envelope_id: value.envelope_id,
            schema_version: EXPORT_ENVELOPE_V1_SCHEMA.into(),
            content_digest,
            source_authority: value.source_authority,
            scope_key: value.scope_key,
            trace_ctx: value.trace_ctx,
            exported_at: value.exported_at,
            records: value.records,
        })
    }
}

impl std::convert::TryFrom<ExportEnvelopeV2> for ExportEnvelopeV3 {
    type Error = ExportEnvelopeError;

    fn try_from(value: ExportEnvelopeV2) -> Result<Self, Self::Error> {
        value.enrich_to_v3()
    }
}

impl std::convert::TryFrom<ExportEnvelopeV3> for ExportEnvelopeV2 {
    type Error = ExportEnvelopeError;

    fn try_from(value: ExportEnvelopeV3) -> Result<Self, Self::Error> {
        value.validate()?;

        let ExportEnvelopeV3 {
            envelope_id,
            source_authority,
            scope_key,
            trace_ctx,
            exported_at,
            export_meta,
            evidence_bundle,
            support_sets,
            contradiction_witnesses,
            retraction_records,
            claim_states_v13,
            intervention_bundles_v14,
            outcome_schemas_v14,
            cohort_contracts_v14,
            counterfactual_slices_v14,
            experiment_cases_v14,
            comparability_matrices_v14,
            decision_traces_v14,
            refuter_suites_v14,
            refuter_results_v14,
            experiment_budgets_v14,
            rollout_decisions_v14,
            rollback_decisions_v14,
            attestation_envelopes_v15,
            trust_root_sets_v15,
            artifact_admission_policies_v15,
            transparency_receipts_v15,
            attestation_revocations_v15,
            attestation_supersessions_v15,
            remote_oracle_leases_v15,
            remote_slice_requests_v15,
            remote_slice_results_v15,
            cross_runtime_replay_tickets_v15,
            dispute_bundles_v15,
            disclosure_policies_v15,
            disclosure_budgets_v15,
            records,
            ..
        } = value;
        let records = records
            .into_iter()
            .map(|record| record.record)
            .collect::<Vec<_>>();
        let content_digest = ExportEnvelopeV2::compute_digest(
            &source_authority,
            &scope_key,
            &records,
            export_meta.as_ref(),
            evidence_bundle.as_ref(),
        )?;

        if !support_sets.is_empty()
            || !contradiction_witnesses.is_empty()
            || !retraction_records.is_empty()
            || !claim_states_v13.is_empty()
            || !intervention_bundles_v14.is_empty()
            || !outcome_schemas_v14.is_empty()
            || !cohort_contracts_v14.is_empty()
            || !counterfactual_slices_v14.is_empty()
            || !experiment_cases_v14.is_empty()
            || !comparability_matrices_v14.is_empty()
            || !decision_traces_v14.is_empty()
            || !refuter_suites_v14.is_empty()
            || !refuter_results_v14.is_empty()
            || !experiment_budgets_v14.is_empty()
            || !rollout_decisions_v14.is_empty()
            || !rollback_decisions_v14.is_empty()
            || !attestation_envelopes_v15.is_empty()
            || !trust_root_sets_v15.is_empty()
            || !artifact_admission_policies_v15.is_empty()
            || !transparency_receipts_v15.is_empty()
            || !attestation_revocations_v15.is_empty()
            || !attestation_supersessions_v15.is_empty()
            || !remote_oracle_leases_v15.is_empty()
            || !remote_slice_requests_v15.is_empty()
            || !remote_slice_results_v15.is_empty()
            || !cross_runtime_replay_tickets_v15.is_empty()
            || !dispute_bundles_v15.is_empty()
            || !disclosure_policies_v15.is_empty()
            || !disclosure_budgets_v15.is_empty()
        {
            warn!("dropping additive v13/v14/v15 artifacts while down-converting ExportEnvelopeV3 to V2");
        }

        Ok(Self {
            envelope_id,
            schema_version: EXPORT_ENVELOPE_V2_SCHEMA.into(),
            content_digest,
            source_authority,
            scope_key,
            trace_ctx,
            exported_at,
            export_meta,
            evidence_bundle,
            records,
        })
    }
}

fn validate_envelope_fields(
    envelope_id: &EnvelopeId,
    schema_version: &str,
    expected_schema_version: &str,
    source_authority: &str,
    scope_key: &ScopeKey,
    records: &[ExportRecord],
) -> Result<(), ExportEnvelopeError> {
    if envelope_id.is_empty() {
        return Err(ExportEnvelopeError::InvalidEnvelope {
            reason: "envelope_id must not be empty".into(),
        });
    }
    if schema_version != expected_schema_version {
        return Err(ExportEnvelopeError::IncompatibleVersion {
            expected: expected_schema_version.into(),
            actual: schema_version.to_string(),
        });
    }
    if source_authority.is_empty() {
        return Err(ExportEnvelopeError::InvalidEnvelope {
            reason: "source_authority must not be empty".into(),
        });
    }
    if scope_key.namespace.is_empty() {
        return Err(ExportEnvelopeError::InvalidEnvelope {
            reason: "scope_key.namespace must not be empty".into(),
        });
    }
    if records.is_empty() {
        return Err(ExportEnvelopeError::InvalidEnvelope {
            reason: "envelope must contain at least one record".into(),
        });
    }
    for (i, record) in records.iter().enumerate() {
        record
            .validate()
            .map_err(|reason| ExportEnvelopeError::InvalidEnvelope {
                reason: format!("record[{i}]: {reason}"),
            })?;
    }
    Ok(())
}

fn digest_computation_failed(reason: impl ToString) -> ExportEnvelopeError {
    ExportEnvelopeError::DigestComputationFailed {
        reason: reason.to_string(),
    }
}

fn sanitized_json_for_digest<T, F>(
    value: &T,
    sanitize: F,
) -> Result<serde_json::Value, ExportEnvelopeError>
where
    T: Serialize,
    F: FnOnce(&mut serde_json::Map<String, serde_json::Value>),
{
    let mut json = serde_json::to_value(value).map_err(digest_computation_failed)?;
    if let serde_json::Value::Object(ref mut map) = json {
        sanitize(map);
    }
    Ok(json)
}

fn compute_digest_inner<R>(
    source_authority: &str,
    scope_key: &ScopeKey,
    records: &[R],
    export_meta: Option<&ForgeExportMeta>,
    evidence_bundle: Option<&EvidenceBundle>,
) -> Result<ContentDigest, ExportEnvelopeError>
where
    R: Serialize,
{
    let mut builder = DigestBuilder::new();
    builder.update_str(source_authority).separator();
    builder
        .update_json(scope_key)
        .map_err(digest_computation_failed)?;
    builder.separator();
    builder
        .update_json(records)
        .map_err(digest_computation_failed)?;
    if let Some(meta) = export_meta {
        builder.separator();
        let meta_value = sanitized_json_for_digest(meta, |map| {
            map.remove("exported_at");
        })?;
        builder
            .update_json(&meta_value)
            .map_err(digest_computation_failed)?;
    }
    if let Some(bundle) = evidence_bundle {
        builder.separator();
        let bundle_value = sanitized_json_for_digest(bundle, |map| {
            map.remove("created_at");
        })?;
        builder
            .update_json(&bundle_value)
            .map_err(digest_computation_failed)?;
    }
    Ok(builder.finalize())
}

fn json_string(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

fn json_enum<T>(value: &serde_json::Value, key: &str) -> Option<T>
where
    T: for<'de> Deserialize<'de>,
{
    json_metadata_field(value, key, true)
}

fn json_object_enum<T>(value: &serde_json::Value, key: &str) -> Option<T>
where
    T: for<'de> Deserialize<'de>,
{
    json_metadata_field(value, key, false)
}

fn json_metadata_field<T>(value: &serde_json::Value, key: &str, required_like: bool) -> Option<T>
where
    T: for<'de> Deserialize<'de>,
{
    let raw = value.get(key)?;
    match serde_json::from_value(raw.clone()) {
        Ok(value) => Some(value),
        Err(err) => {
            let expectation = if required_like { "enum" } else { "object" };
            warn!(
                key = key,
                value_kind = raw.to_string(),
                target_type = type_name::<T>(),
                expected = expectation,
                error = %err,
                "present but unreadable kernel metadata field dropped in V2->V3 enrichment"
            );
            None
        }
    }
}

fn validate_envelope_fields_v3(
    envelope_id: &EnvelopeId,
    schema_version: &str,
    expected_schema_version: &str,
    source_authority: &str,
    scope_key: &ScopeKey,
    records: &[ExportRecordV3],
) -> Result<(), ExportEnvelopeError> {
    if envelope_id.is_empty() {
        return Err(ExportEnvelopeError::InvalidEnvelope {
            reason: "envelope_id must not be empty".into(),
        });
    }
    if schema_version != expected_schema_version {
        return Err(ExportEnvelopeError::IncompatibleVersion {
            expected: expected_schema_version.into(),
            actual: schema_version.to_string(),
        });
    }
    if source_authority.is_empty() {
        return Err(ExportEnvelopeError::InvalidEnvelope {
            reason: "source_authority must not be empty".into(),
        });
    }
    if scope_key.namespace.is_empty() {
        return Err(ExportEnvelopeError::InvalidEnvelope {
            reason: "scope_key.namespace must not be empty".into(),
        });
    }
    if records.is_empty() {
        return Err(ExportEnvelopeError::InvalidEnvelope {
            reason: "envelope must contain at least one record".into(),
        });
    }
    for (i, record) in records.iter().enumerate() {
        record
            .record
            .validate()
            .map_err(|reason| ExportEnvelopeError::InvalidEnvelope {
                reason: format!("record[{i}]: {reason}"),
            })?;
    }
    Ok(())
}

/// A single record within an export envelope.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExportRecord {
    /// A claim (knowledge assertion) from verified Forge output.
    Claim(ExportClaim),
    /// A relation (edge between entities) from verified Forge output.
    Relation(ExportRelation),
    /// An episode (causal record) from Forge experiments.
    Episode(ExportEpisode),
    /// An entity alias/merge suggestion from entity resolution.
    EntityAlias(ExportEntityAlias),
    /// An evidence reference for audit purposes.
    EvidenceRef(ExportEvidenceRef),
}

impl ExportRecord {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Claim(c) => {
                if c.predicate.is_empty() {
                    return Err("claim predicate must not be empty".into());
                }
                if c.subject_entity_id.is_empty() {
                    return Err("claim subject_entity_id must not be empty".into());
                }
                if !c.confidence.is_finite() {
                    return Err("claim confidence must be finite".into());
                }
            }
            Self::Relation(r) => {
                if r.predicate.is_empty() {
                    return Err("relation predicate must not be empty".into());
                }
                if r.subject_entity_id.is_empty() {
                    return Err("relation subject_entity_id must not be empty".into());
                }
                if !r.confidence.is_finite() {
                    return Err("relation confidence must be finite".into());
                }
            }
            Self::Episode(e) => {
                if e.effect_type.is_empty() {
                    return Err("episode effect_type must not be empty".into());
                }
                if !e.confidence.is_finite() {
                    return Err("episode confidence must be finite".into());
                }
            }
            Self::EntityAlias(a) => {
                if a.canonical_entity_id.is_empty() {
                    return Err("entity alias canonical_entity_id must not be empty".into());
                }
                if a.alias_text.is_empty() {
                    return Err("entity alias text must not be empty".into());
                }
                if !a.confidence.is_finite() {
                    return Err("entity alias confidence must be finite".into());
                }
            }
            Self::EvidenceRef(e) => {
                if e.claim_id.is_empty() {
                    return Err("evidence ref claim_id must not be empty".into());
                }
                if e.fetch_handle.is_empty() {
                    return Err("evidence ref fetch_handle must not be empty".into());
                }
            }
        }
        Ok(())
    }
}

/// A claim exported from Forge.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExportClaim {
    /// Optional pre-assigned claim ID (Forge may leave this for memory to assign).
    pub claim_id: Option<ClaimId>,
    /// Optional pre-assigned claim version ID.
    ///
    /// V2 exporters use this to preserve exact version identity end-to-end.
    /// V1 exporters normally leave it unset and let the bridge mint a version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_version_id: Option<ClaimVersionId>,
    /// The entity this claim is about.
    pub subject_entity_id: EntityId,
    /// The predicate (e.g. "has_property", "is_type_of").
    pub predicate: String,
    /// The object anchor (value, entity ref, or literal).
    pub object_anchor: serde_json::Value,
    /// Validity start time (ISO 8601). None = always valid.
    pub valid_from: Option<String>,
    /// Validity end time (ISO 8601). None = open-ended.
    pub valid_to: Option<String>,
    /// Source confidence (0.0 - 1.0).
    pub confidence: f32,
    /// The content text for embedding/search.
    pub content: String,
    /// Projection family (e.g. "forge_verification", "manual").
    pub projection_family: String,
    /// Claim-level supersession pointer retained for compatibility/audit.
    pub supersedes_claim_id: Option<ClaimId>,
    /// Version-level supersession pointer when the exporter knows it.
    #[serde(default)]
    pub supersedes_claim_version_id: Option<ClaimVersionId>,
    /// Additional metadata.
    pub metadata: Option<serde_json::Value>,
}

/// A relation exported from Forge.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExportRelation {
    /// Optional pre-assigned relation version identity.
    ///
    /// V2 exporters use this to preserve exact version identity end-to-end.
    /// V1 exporters normally leave it unset and let the bridge mint a version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation_version_id: Option<RelationVersionId>,
    /// The subject entity.
    pub subject_entity_id: EntityId,
    /// The relation predicate (e.g. "depends_on", "authored_by").
    pub predicate: String,
    /// The object anchor (typically another entity ID or literal).
    pub object_anchor: serde_json::Value,
    /// Validity start time (ISO 8601).
    pub valid_from: Option<String>,
    /// Validity end time (ISO 8601).
    pub valid_to: Option<String>,
    /// Source confidence.
    pub confidence: f32,
    /// Projection family.
    pub projection_family: String,
    /// Linked claim or episode.
    pub source_claim_id: Option<ClaimId>,
    pub source_episode_id: Option<EpisodeId>,
    /// Whether this supersedes a previous relation version.
    pub supersedes_relation_version_id: Option<RelationVersionId>,
    /// Additional metadata.
    pub metadata: Option<serde_json::Value>,
}

/// An episode exported from Forge.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExportEpisode {
    /// Episode identity.
    pub episode_id: Option<EpisodeId>,
    /// Linked document.
    pub document_id: String,
    /// Cause IDs (facts/chunks/messages).
    pub cause_ids: Vec<String>,
    /// Type of effect.
    pub effect_type: String,
    /// Outcome assessment.
    pub outcome: String,
    /// Confidence in causal link.
    pub confidence: f32,
    /// Experiment ID if experimentally verified.
    pub experiment_id: Option<String>,
    /// Additional metadata.
    pub metadata: Option<serde_json::Value>,
}

/// An entity alias suggestion from Forge.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExportEntityAlias {
    /// The canonical entity this alias maps to.
    pub canonical_entity_id: EntityId,
    /// The alias text.
    pub alias_text: String,
    /// Source of the alias (e.g. "forge_extraction", "manual").
    pub alias_source: String,
    /// Match evidence (opaque blob for audit).
    pub match_evidence: Option<serde_json::Value>,
    /// Confidence in the alias mapping.
    pub confidence: f32,
    /// Scope semantics for this alias.
    pub scope: Option<ScopeKey>,
    /// If this alias participates in a merge lineage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by_entity_id: Option<EntityId>,
    /// If this alias came from a split lineage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_from_entity_id: Option<EntityId>,
}

/// An evidence reference for audit purposes.
///
/// Evidence refs are opaque by default. Their fetch path is explicit
/// and audit-only — no automatic dereferencing in normal query paths.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExportEvidenceRef {
    /// The claim this evidence supports.
    pub claim_id: ClaimId,
    /// Version-local linkage (nullable, per delta §5.5).
    pub claim_version_id: Option<ClaimVersionId>,
    /// Canonical raw-evidence fetch handle (opaque URI or key).
    pub fetch_handle: String,
    /// Source authority for the evidence.
    pub source_authority: String,
    /// Additional metadata for audit.
    pub metadata: Option<serde_json::Value>,
}

#[cfg(test)]
#[path = "envelope_tests.rs"]
mod tests;
