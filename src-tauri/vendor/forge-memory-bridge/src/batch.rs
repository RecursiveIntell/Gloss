#![allow(deprecated)]

//! Bridge-owned projection import batch types.
//!
//! `ProjectionImportBatchV1` remains available only as a compatibility output.
//! `ProjectionImportBatchV2` remains the normalized storage/import shape for
//! compatibility callers.
//! The canonical bridge output for the kernel-ready lane is
//! `ProjectionImportBatchV3`.
//!
//! ## Phase status: current / implemented now

use schemars::JsonSchema;
use semantic_memory_forge::{
    ClaimStateV13, ContradictionWitnessV1, EpisodeBundleV1, EvidenceBundle, ExecutionContextV1,
    ExportRecordSemanticsV3, ForgeExportMeta, RetractionRecordV1, SupportSetV1,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use stack_ids::{
    ClaimId, ClaimVersionId, ContentDigest, EntityId, EnvelopeId, EpisodeId, RelationVersionId,
    ScopeKey, TraceCtx,
};

/// Legacy compatibility import-batch schema version emitted by the bridge.
///
/// This is distinct from the source export envelope schema version. The latter
/// is preserved in [`ProjectionImportBatchV1::export_schema_version`].
///
/// Phase status: migration-only
/// Removal condition: remove when all consumers have migrated to `ProjectionImportBatchV3`
#[deprecated(
    since = "0.2.0",
    note = "ProjectionImportBatchV1 is compatibility-only. Use ProjectionImportBatchV3 for the canonical bridge output."
)]
pub const PROJECTION_IMPORT_BATCH_V1_SCHEMA: &str = "projection_import_batch_v1";
/// Canonical import-batch schema version emitted by the bridge.
pub const PROJECTION_IMPORT_BATCH_V2_SCHEMA: &str = "projection_import_batch_v2";
/// Rich import-batch schema version emitted by the bridge for the kernel path.
pub const PROJECTION_IMPORT_BATCH_V3_SCHEMA: &str = "projection_import_batch_v3";

/// A projection import batch produced by the bridge.
///
/// This is the unit of atomicity for projection imports. All records
/// are committed together or rolled back entirely.
///
/// Temporal provenance is intentionally split:
/// - `source_exported_at` = when the source envelope was emitted
/// - `transformed_at` = when the bridge produced this batch
///
/// The importing store assigns authoritative imported `recorded_at`
/// when it commits the rows.
///
/// Phase status: migration-only
/// Removal condition: remove when all consumers have migrated to `ProjectionImportBatchV3`
#[deprecated(
    since = "0.2.0",
    note = "ProjectionImportBatchV1 is compatibility-only. Use ProjectionImportBatchV3 for the canonical bridge output."
)]
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProjectionImportBatchV1 {
    /// Source envelope ID (for provenance).
    pub source_envelope_id: EnvelopeId,
    /// Import-side schema version this batch conforms to (I005).
    ///
    /// For V1, the bridge emits the canonical import-side token
    /// `projection_import_batch_v1`. The source export schema version is
    /// preserved separately in `export_schema_version`.
    pub schema_version: String,
    /// The original export-side schema version from the source envelope.
    ///
    /// Recorded for provenance; may differ from `schema_version` when the
    /// bridge maps across versions.
    #[serde(default)]
    pub export_schema_version: Option<String>,
    /// Content digest carried through from the source envelope.
    pub content_digest: ContentDigest,
    /// Source authority (e.g. "forge").
    pub source_authority: String,
    /// Target scope.
    pub scope_key: ScopeKey,
    /// Trace context carried through from the source.
    pub trace_ctx: Option<TraceCtx>,
    /// When the source envelope was exported.
    pub source_exported_at: String,
    /// When this batch was produced by the bridge.
    pub transformed_at: String,
    /// The import records, ready for memory ingestion.
    pub records: Vec<ImportProjectionRecord>,
}

/// A projection import batch produced by the bridge for the V2 contract.
///
/// V2 preserves the V1 projection records and additionally carries the
/// export-time metadata plus the canonical evidence bundle payload that must
/// survive into import receipts.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProjectionImportBatchV2 {
    /// Source envelope ID (for provenance).
    pub source_envelope_id: EnvelopeId,
    /// Import-side schema version this batch conforms to.
    pub schema_version: String,
    /// The original export-side schema version from the source envelope.
    #[serde(default)]
    pub export_schema_version: Option<String>,
    /// Content digest carried through from the source envelope.
    pub content_digest: ContentDigest,
    /// Source authority (e.g. "forge").
    pub source_authority: String,
    /// Target scope.
    pub scope_key: ScopeKey,
    /// Trace context carried through from the source.
    pub trace_ctx: Option<TraceCtx>,
    /// When the source envelope was exported.
    pub source_exported_at: String,
    /// When this batch was produced by the bridge.
    pub transformed_at: String,
    /// Optional export metadata from the source envelope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_meta: Option<ForgeExportMeta>,
    /// Optional canonical evidence bundle from the source envelope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_bundle: Option<EvidenceBundle>,
    /// Canonical v9 episode bundle proof surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_bundle: Option<EpisodeBundleV1>,
    /// Canonical v9 execution context artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_context: Option<ExecutionContextV1>,
    /// The import records, ready for memory ingestion.
    pub records: Vec<ImportProjectionRecord>,
}

/// Canonical kernel-oriented projection import batch produced by the bridge.
///
/// V3 extends V2 with rich record semantics, support algebra artifacts,
/// contradiction witnesses, retraction lineage, and v14/v15 experimental
/// and exchange surfaces preserved verbatim from the Forge export.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProjectionImportBatchV3 {
    /// Source envelope ID (for provenance).
    pub source_envelope_id: EnvelopeId,
    /// Import-side schema version this batch conforms to.
    pub schema_version: String,
    /// The original export-side schema version from the source envelope.
    #[serde(default)]
    pub export_schema_version: Option<String>,
    /// Content digest carried through from the source envelope.
    pub content_digest: ContentDigest,
    /// Source authority (e.g. "forge").
    pub source_authority: String,
    /// Target scope.
    pub scope_key: ScopeKey,
    /// Trace context carried through from the source.
    pub trace_ctx: Option<TraceCtx>,
    /// When the source envelope was exported.
    pub source_exported_at: String,
    /// When this batch was produced by the bridge.
    pub transformed_at: String,
    /// Optional export metadata from the source envelope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_meta: Option<ForgeExportMeta>,
    /// Optional canonical evidence bundle from the source envelope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_bundle: Option<EvidenceBundle>,
    /// Canonical v9 episode bundle proof surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_bundle: Option<EpisodeBundleV1>,
    /// Canonical v9 execution context artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_context: Option<ExecutionContextV1>,
    /// Additive v13 support algebra artifacts preserved verbatim from Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub support_sets: Vec<SupportSetV1>,
    /// Additive v13 contradiction witnesses preserved verbatim from Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contradiction_witnesses: Vec<ContradictionWitnessV1>,
    /// Additive v13 retraction lineage artifacts preserved verbatim from Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retraction_records: Vec<RetractionRecordV1>,
    /// Additive v13 claim-state artifacts preserved verbatim from Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claim_states_v13: Vec<ClaimStateV13>,
    /// Additive v14 intervention and control artifacts preserved verbatim from Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intervention_bundles_v14: Vec<Value>,
    /// V14 outcome schema definitions preserved verbatim from Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outcome_schemas_v14: Vec<Value>,
    /// V14 cohort contract artifacts preserved verbatim from Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cohort_contracts_v14: Vec<Value>,
    /// V14 counterfactual slice artifacts preserved verbatim from Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub counterfactual_slices_v14: Vec<Value>,
    /// V14 experiment case artifacts preserved verbatim from Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub experiment_cases_v14: Vec<Value>,
    /// V14 comparability matrix artifacts preserved verbatim from Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comparability_matrices_v14: Vec<Value>,
    /// V14 decision trace artifacts preserved verbatim from Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decision_traces_v14: Vec<Value>,
    /// V14 refuter suite definitions preserved verbatim from Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refuter_suites_v14: Vec<Value>,
    /// V14 refuter result artifacts preserved verbatim from Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refuter_results_v14: Vec<Value>,
    /// V14 experiment budget artifacts preserved verbatim from Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub experiment_budgets_v14: Vec<Value>,
    /// V14 rollout decision artifacts preserved verbatim from Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rollout_decisions_v14: Vec<Value>,
    /// V14 rollback decision artifacts preserved verbatim from Forge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rollback_decisions_v14: Vec<Value>,
    /// Additive v15 exchange and remote-admission artifacts preserved verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attestation_envelopes_v15: Vec<Value>,
    /// V15 trust root set artifacts preserved verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trust_root_sets_v15: Vec<Value>,
    /// V15 artifact admission policy artifacts preserved verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_admission_policies_v15: Vec<Value>,
    /// V15 transparency receipt artifacts preserved verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transparency_receipts_v15: Vec<Value>,
    /// V15 attestation revocation artifacts preserved verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attestation_revocations_v15: Vec<Value>,
    /// V15 attestation supersession artifacts preserved verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attestation_supersessions_v15: Vec<Value>,
    /// V15 remote oracle lease artifacts preserved verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remote_oracle_leases_v15: Vec<Value>,
    /// V15 remote slice request artifacts preserved verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remote_slice_requests_v15: Vec<Value>,
    /// V15 remote slice result artifacts preserved verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remote_slice_results_v15: Vec<Value>,
    /// V15 cross-runtime replay ticket artifacts preserved verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cross_runtime_replay_tickets_v15: Vec<Value>,
    /// V15 dispute bundle artifacts preserved verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dispute_bundles_v15: Vec<Value>,
    /// V15 disclosure policy artifacts preserved verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disclosure_policies_v15: Vec<Value>,
    /// V15 disclosure budget artifacts preserved verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disclosure_budgets_v15: Vec<Value>,
    /// Rich import records preserving export-time semantics for kernel compilation.
    pub records: Vec<ImportProjectionRecordV3>,
}

/// A single import record enriched with export-time semantics for kernel compilation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImportProjectionRecordV3 {
    pub record: ImportProjectionRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantics: Option<ExportRecordSemanticsV3>,
}

impl From<ProjectionImportBatchV1> for ProjectionImportBatchV2 {
    fn from(value: ProjectionImportBatchV1) -> Self {
        Self {
            source_envelope_id: value.source_envelope_id,
            schema_version: PROJECTION_IMPORT_BATCH_V2_SCHEMA.into(),
            export_schema_version: value.export_schema_version,
            content_digest: value.content_digest,
            source_authority: value.source_authority,
            scope_key: value.scope_key,
            trace_ctx: value.trace_ctx,
            source_exported_at: value.source_exported_at,
            transformed_at: value.transformed_at,
            export_meta: None,
            evidence_bundle: None,
            episode_bundle: None,
            execution_context: None,
            records: value.records,
        }
    }
}

impl From<ProjectionImportBatchV2> for ProjectionImportBatchV1 {
    fn from(value: ProjectionImportBatchV2) -> Self {
        Self {
            source_envelope_id: value.source_envelope_id,
            schema_version: PROJECTION_IMPORT_BATCH_V1_SCHEMA.into(),
            export_schema_version: value.export_schema_version,
            content_digest: value.content_digest,
            source_authority: value.source_authority,
            scope_key: value.scope_key,
            trace_ctx: value.trace_ctx,
            source_exported_at: value.source_exported_at,
            transformed_at: value.transformed_at,
            records: value.records,
        }
    }
}

/// A single record in a projection import batch.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImportProjectionRecord {
    /// A claim projection version to import.
    ClaimVersion(ImportClaimVersion),
    /// A relation version to import.
    RelationVersion(ImportRelationVersion),
    /// An episode to import.
    Episode(ImportEpisodeRecord),
    /// An entity alias to import.
    EntityAlias(ImportEntityAlias),
    /// An evidence reference to import.
    EvidenceRef(ImportEvidenceRef),
}

/// A claim projection version ready for import.
///
/// ## Required fields (from MASTER_SUPPORTING_DELTA §5.1 and spec)
///
/// All fields listed here are mandatory in the storage model. Optional
/// fields are explicitly marked with `Option<>`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImportClaimVersion {
    /// Claim identity (stable across versions).
    pub claim_id: ClaimId,
    /// Version identity (unique per mutation).
    pub claim_version_id: ClaimVersionId,
    /// Current state of this claim version.
    pub claim_state: ClaimState,
    /// Projection family (e.g. "forge_verification").
    pub projection_family: String,
    /// The entity this claim is about.
    pub subject_entity_id: EntityId,
    /// The predicate.
    pub predicate: String,
    /// The object anchor.
    pub object_anchor: serde_json::Value,
    /// Target scope.
    pub scope_key: ScopeKey,
    /// Validity start (ISO 8601).
    pub valid_from: Option<String>,
    /// Validity end (ISO 8601). None = open-ended.
    pub valid_to: Option<String>,
    /// Whether this is the preferred open version for its logical key.
    pub preferred_open: bool,
    /// Source envelope provenance.
    pub source_envelope_id: EnvelopeId,
    /// Source authority.
    pub source_authority: String,
    /// Trace context.
    pub trace_ctx: Option<TraceCtx>,
    /// Freshness status.
    pub freshness: ProjectionFreshness,
    /// Contradiction status.
    pub contradiction_status: ContradictionStatus,
    /// Which previous version this supersedes, if any.
    pub supersedes_claim_version_id: Option<ClaimVersionId>,
    /// The content text for embedding/search.
    pub content: String,
    /// Source confidence.
    pub confidence: f32,
    /// Additional metadata.
    pub metadata: Option<serde_json::Value>,
}

/// A relation version ready for import.
///
/// Preserves audit-grade metadata parity with claim versions
/// (MASTER_SUPPORTING_DELTA §5.2).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImportRelationVersion {
    /// Relation version identity.
    pub relation_version_id: RelationVersionId,
    /// The subject entity.
    pub subject_entity_id: EntityId,
    /// The relation predicate.
    pub predicate: String,
    /// The object anchor.
    pub object_anchor: serde_json::Value,
    /// Target scope.
    pub scope_key: ScopeKey,
    /// Linked claim or episode.
    pub claim_id: Option<ClaimId>,
    pub source_episode_id: Option<EpisodeId>,
    /// Validity times.
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    /// Preferred open flag.
    pub preferred_open: bool,
    /// Which previous version this supersedes.
    pub supersedes_relation_version_id: Option<RelationVersionId>,
    /// Contradiction status.
    pub contradiction_status: ContradictionStatus,
    /// Source confidence.
    pub source_confidence: f32,
    /// Projection family.
    pub projection_family: String,
    /// Source envelope provenance.
    pub source_envelope_id: EnvelopeId,
    /// Source authority.
    pub source_authority: String,
    /// Trace context.
    pub trace_ctx: Option<TraceCtx>,
    /// Freshness.
    pub freshness: ProjectionFreshness,
    /// Additional metadata.
    pub metadata: Option<serde_json::Value>,
}

/// An episode ready for import.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImportEpisodeRecord {
    /// Episode identity.
    pub episode_id: EpisodeId,
    /// Linked document.
    pub document_id: String,
    /// Cause IDs.
    pub cause_ids: Vec<String>,
    /// Effect type.
    pub effect_type: String,
    /// Outcome.
    pub outcome: String,
    /// Confidence.
    pub confidence: f32,
    /// Experiment ID.
    pub experiment_id: Option<String>,
    /// Source envelope provenance.
    pub source_envelope_id: EnvelopeId,
    /// Source authority.
    pub source_authority: String,
    /// Trace context.
    pub trace_ctx: Option<TraceCtx>,
    /// Additional metadata.
    pub metadata: Option<serde_json::Value>,
}

/// An entity alias ready for import.
///
/// Includes explicit scope semantics and durable review state
/// (MASTER_SUPPORTING_DELTA §5.3, §5.4).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImportEntityAlias {
    /// The canonical entity.
    pub canonical_entity_id: EntityId,
    /// Alias text.
    pub alias_text: String,
    /// Source of the alias.
    pub alias_source: String,
    /// Match evidence blob (opaque).
    pub match_evidence: Option<serde_json::Value>,
    /// Confidence.
    pub confidence: f32,
    /// Merge decision provenance.
    pub merge_decision: MergeDecision,
    /// Explicit scope for this alias.
    pub scope: ScopeKey,
    /// Review state.
    pub review_state: ReviewState,
    /// Whether this has been human-confirmed.
    pub is_human_confirmed: bool,
    /// Whether this is final (only set by human review or audited migration).
    pub is_human_confirmed_final: bool,
    /// If this entity was superseded by a merge.
    pub superseded_by_entity_id: Option<EntityId>,
    /// Split history pointer.
    pub split_from_entity_id: Option<EntityId>,
    /// Source envelope provenance.
    pub source_envelope_id: EnvelopeId,
}

/// An evidence reference ready for import.
///
/// Evidence refs are opaque by default with explicit audit dereference only
/// (MASTER_SUPPORTING_DELTA §5.5).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImportEvidenceRef {
    /// The claim this evidence supports.
    pub claim_id: ClaimId,
    /// Version-local linkage.
    pub claim_version_id: Option<ClaimVersionId>,
    /// Canonical raw-evidence fetch handle.
    pub fetch_handle: String,
    /// Source authority.
    pub source_authority: String,
    /// Source envelope provenance.
    pub source_envelope_id: EnvelopeId,
    /// Additional audit metadata.
    pub metadata: Option<serde_json::Value>,
}

/// Lifecycle state of a claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClaimState {
    /// Active and current.
    Active,
    /// Superseded by a newer version.
    Superseded,
    /// Retracted (found to be incorrect).
    Retracted,
    /// Archived (no longer relevant but preserved).
    Archived,
    /// Pending review.
    PendingReview,
    /// Disputed (contradicted but not resolved).
    Disputed,
}

impl ClaimState {
    /// Returns the stable snake_case wire label for this claim state.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Retracted => "retracted",
            Self::Archived => "archived",
            Self::PendingReview => "pending_review",
            Self::Disputed => "disputed",
        }
    }
}

/// Projection freshness status.
///
/// Extended from the original enum per spec requirements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionFreshness {
    /// Projection was recently imported and is current.
    Current,
    /// Projection exists but is older than the staleness threshold.
    Stale,
    /// A newer version has superseded this projection's data.
    Superseded,
    /// The last import attempt failed.
    ImportFailed,
    /// No import has ever been recorded.
    NeverImported,
    /// Import is lagging behind the source.
    ImportLagging,
}

impl ProjectionFreshness {
    /// Returns the stable snake_case wire label for this freshness status.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Stale => "stale",
            Self::Superseded => "superseded",
            Self::ImportFailed => "import_failed",
            Self::NeverImported => "never_imported",
            Self::ImportLagging => "import_lagging",
        }
    }
}

/// Contradiction status for claims and relations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContradictionStatus {
    /// No contradictions known.
    None,
    /// Possible contradiction detected (structural check only).
    PossibleContradiction { description: String },
    /// Confirmed contradiction.
    Confirmed { contradicted_by: ClaimId },
    /// Resolved (e.g. one claim retracted).
    Resolved { resolution: String },
}

/// Merge decision provenance for entity aliases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MergeDecision {
    /// Automated merge (must not set human_confirmed_final).
    Automated { algorithm: String },
    /// Human-reviewed merge.
    HumanReviewed { reviewer: String, at: String },
    /// Pending human review.
    PendingReview,
    /// Rejected (not a merge).
    Rejected { reason: String },
}

/// Durable review state for alias/merge decisions.
///
/// Must survive restart and be queryable (MASTER_SUPPORTING_DELTA §5.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    /// Not yet reviewed.
    Unreviewed,
    /// Awaiting human review.
    PendingReview,
    /// Reviewed and approved.
    Approved { by: String, at: String },
    /// Reviewed and rejected.
    Rejected {
        by: String,
        at: String,
        reason: String,
    },
}
