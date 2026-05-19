#![allow(deprecated)]
#![cfg_attr(test, allow(clippy::expect_used))]

//! # semantic-memory-forge
//!
//! Forge owns **raw verification truth**.
//!
//! This crate provides the evidence bundle schema, export envelope ownership,
//! causal estimation metadata, and refutation structure required by the
//! canonical verification pipeline.
//!
//! ## Authority
//!
//! - Forge is authoritative for raw verification truth and export envelopes.
//! - Memory (`semantic-memory`) is authoritative for queryable projected truth.
//! - Bridge (`forge-memory-bridge`) is authoritative only for transformation.
//! - Runtime (`knowledge-runtime`) is authoritative only for query planning/merge.
//!
//! ## Evidence Bundle Contract
//!
//! Every causal/effect verification produces an `EvidenceBundle` that preserves
//! the full methodological chain: question, design, estimation, refutation,
//! and raw receipt handles.
//!
//! ## Direct-Write Law
//!
//! Direct memory write-through is **not** the normal path. Any emergency
//! direct-write facility must remain exceptional, auditable, and disabled
//! by default in release builds.

pub mod bundle;
pub mod envelope;
pub mod estimator;
pub mod tool_receipt;
pub mod v11;
pub mod v13;
pub mod v14;
pub mod v9;

pub use bundle::{
    CausalQuestion, ComparabilitySnapshot, EvidenceBundle, EvidenceBundleId, OutcomeSpec,
    PromotionState, RefutationArtifactRecord, RefutationAttempt, RefutationResult, TreatmentSpec,
    VerificationLifecycleState, VerificationSummary, VerificationTrialRecord,
    VerificationTrialSide,
};
pub use envelope::{
    CausalRoleHint, ConstraintSeedKind, ExportAuthority, ExportClaim, ExportConfidenceClass,
    ExportEntityAlias, ExportEnvelopeError, ExportEnvelopeV1, ExportEnvelopeV2, ExportEnvelopeV3,
    ExportEpisode, ExportEvidenceRef, ExportRecord, ExportRecordSemanticsV3, ExportRecordV3,
    ExportRelation, ForgeExportMeta, NuisanceSnapshot, ProjectionVisibilityClass,
    EXPORT_ENVELOPE_V1_SCHEMA, EXPORT_ENVELOPE_V2_SCHEMA, EXPORT_ENVELOPE_V3_SCHEMA,
};
pub use estimator::{EnvironmentFingerprint, EstimatorKind, EstimatorMeta, SidecarExecution};
pub use tool_receipt::{
    ForgeToolBudgetContext, ForgeToolReceiptV1, ForgeToolReceiptV2, FORGE_TOOL_RECEIPT_V1_SCHEMA,
    FORGE_TOOL_RECEIPT_V2_SCHEMA,
};
pub use v11::{
    CausalAttributionBundleV1, CausalContributorV1, CausalDirectionV1, CausalRoleV1,
    CertificateArtifactV1, CertificateKindV1, ClaimStateV1, DegradationActionV1, DegradationKindV1,
    DegradationRecordV1, EvidenceAdmissibilityV1, ExactnessBudgetV1, ExactnessEscalationRuleV1,
    ExactnessLevelV1, OracleSliceContractV1, RefutationArtifactV1, RefutationOutcomeV1,
    SemanticDiffV1, SemanticViewV1, SemanticsProfileV1, TruthStateV1, WitnessArtifactV1,
    CAUSAL_ATTRIBUTION_BUNDLE_V1_SCHEMA, CERTIFICATE_ARTIFACT_V1_SCHEMA, CLAIM_STATE_V1_SCHEMA,
    DEGRADATION_RECORD_V1_SCHEMA, EXACTNESS_BUDGET_V1_SCHEMA, ORACLE_SLICE_CONTRACT_V1_SCHEMA,
    REFUTATION_ARTIFACT_V1_SCHEMA, SEMANTICS_PROFILE_V1_SCHEMA, SEMANTIC_DIFF_V1_SCHEMA,
    WITNESS_ARTIFACT_V1_SCHEMA,
};
pub use v13::{
    BilatticeTruthV1, ClaimStateV13, ContradictionWitnessV1, QualityVectorV1, RetractionRecordV1,
    SupportExprV1, SupportPolarityV1, SupportProvenanceKindV1, SupportSetV1, SupportTokenV1,
    BILATTICE_TRUTH_V1_SCHEMA, CLAIM_STATE_V13_SCHEMA, CONTRADICTION_WITNESS_V1_SCHEMA,
    RETRACTION_RECORD_V1_SCHEMA, SUPPORT_SET_V1_SCHEMA,
};
pub use v14::{
    CohortContractV1, CounterfactualSliceV1, InterventionBundleV1, OutcomeSchemaV1,
    COHORT_CONTRACT_V1_SCHEMA, COUNTERFACTUAL_SLICE_V1_SCHEMA, INTERVENTION_BUNDLE_V1_SCHEMA,
    OUTCOME_SCHEMA_V1_SCHEMA,
};
pub use v9::{
    DispatchOutcomeV1, EpisodeBundleV1, ExecutionContextV1, EPISODE_BUNDLE_V1_SCHEMA,
    EXECUTION_CONTEXT_V1_SCHEMA,
};
