//! Opaque ID newtypes for cross-crate identity.
//!
//! Each newtype wraps a `String` and is intentionally opaque — callers
//! should not parse the inner value. IDs are assigned by their respective
//! authority crates; `stack-ids` only provides the type contract.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Macro to generate an opaque string-wrapper ID type with standard impls.
macro_rules! define_id {
    (
        $(#[$meta:meta])*
        $name:ident
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// Create from any string-like value.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Generate a new random UUID v4 ID.
            pub fn generate() -> Self {
                Self(uuid::Uuid::new_v4().to_string())
            }

            /// Borrow as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Returns true if the inner string is empty.
            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl FromStr for $name {
            type Err = std::convert::Infallible;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Ok(Self::new(value))
            }
        }
    };
}

define_id!(
    /// Opaque identifier for an import/export envelope.
    ///
    /// Assigned by the exporting authority (e.g. Forge). Stable across
    /// re-exports of the same logical unit.
    EnvelopeId
);

define_id!(
    /// Opaque identifier for a claim (knowledge assertion).
    ///
    /// Assigned by the projection importer. Stable within a claim lineage.
    ClaimId
);

define_id!(
    /// Opaque identifier for a specific version of a claim.
    ///
    /// Each mutation to a claim's validity, status, or content produces
    /// a new version with a new `ClaimVersionId`. The `ClaimId` remains
    /// stable across versions.
    ClaimVersionId
);

define_id!(
    /// Opaque identifier for an entity (person, concept, code unit, etc.).
    ///
    /// The inner string is intentionally unstructured. Use domain-specific
    /// constructors to create these (e.g. code_entity_id).
    EntityId
);

define_id!(
    /// Opaque identifier for an episode (causal record).
    ///
    /// Assigned by the episode creator. Stable within the episode's lifecycle.
    EpisodeId
);

define_id!(
    /// Opaque identifier for a logical retry family within one retry-owner boundary.
    ///
    /// One `AttemptId` exists per logical retry family. Retries inside that
    /// boundary produce new `TrialId`s, NOT new `AttemptId`s. A new `AttemptId`
    /// is created only when the retry owner changes (e.g. node-level retry
    /// after transport retries are exhausted) or on explicit replay/re-enqueue.
    AttemptId
);

define_id!(
    /// Opaque identifier for a concrete execution within a logical retry family.
    ///
    /// Every retry within one owner boundary creates a new `TrialId` under
    /// the same `AttemptId`. Each `TrialId` is globally unique (UUID v4).
    TrialId
);

define_id!(
    /// Opaque identifier for a stored artifact (patch, snapshot, file).
    ///
    /// Assigned by the artifact store. Stable across reads.
    ArtifactId
);

define_id!(
    /// Opaque identifier for a projection instance.
    ///
    /// Identifies a specific derived view (entity registry entry, temporal
    /// claim set, etc.) within a scope.
    ProjectionId
);

define_id!(
    /// Opaque identifier for a relation (edge between entities).
    ///
    /// Assigned by the projection importer. Stable within a relation lineage.
    /// Each mutation produces a new `RelationVersionId` under the same `RelationId`.
    RelationId
);

define_id!(
    /// Opaque identifier for a relation version.
    ///
    /// Each mutation to a relation produces a new version with a new
    /// `RelationVersionId`. The `RelationId` remains stable across versions.
    RelationVersionId
);

define_id!(
    /// Opaque identifier for an import batch produced by the bridge.
    ///
    /// Assigned by the bridge transformation pipeline. Unique per batch.
    ImportBatchId
);

define_id!(
    /// Opaque identifier for a claim family spanning multiple claim versions.
    ///
    /// Assigned by the authoritative exporter when version lineage is known.
    ClaimFamilyId
);

define_id!(
    /// Opaque identifier for a higher-order assertion group.
    ///
    /// Used to preserve joint assertion meaning across export, bridge, and
    /// deterministic compilation. The bridge preserves it; it does not invent it.
    AssertionGroupId
);

define_id!(
    /// Opaque identifier for a relation group exported by the authority.
    ///
    /// Relation groups allow the compiler to recover hyperedge structure without
    /// flattening upstream semantics into pairwise folklore.
    RelationGroupId
);

define_id!(
    /// Opaque identifier for a joint evidence group.
    ///
    /// Used to tie multiple observations or assertions to a shared evidence basis.
    JointEvidenceGroupId
);

define_id!(
    /// Opaque identifier for a contradiction candidate group.
    ///
    /// Assigned upstream when the exporting authority exposes contradiction seed
    /// structure. Kernel/runtime layers must not synthesize it from absence.
    ContradictionGroupId
);

define_id!(
    /// Opaque identifier for a mutual exclusion or constraint seed group.
    ConstraintGroupId
);

define_id!(
    /// Opaque identifier for a recursive kernel run.
    ///
    /// Assigned by the kernel execution layer for one bounded, rebuildable run.
    KernelRunId
);

define_id!(
    /// Opaque identifier for a compiled constraint unit.
    ConstraintId
);

define_id!(
    /// Opaque identifier for a compiled hyperedge recovered from upstream grouping semantics.
    HyperedgeId
);

define_id!(
    /// Opaque identifier for a residual artifact emitted by bounded execution.
    ResidualId
);

define_id!(
    /// Opaque identifier for a stable syndrome signature emitted by the kernel.
    SyndromeId
);

define_id!(
    /// Opaque identifier for a witness artifact produced by execution or oracles.
    WitnessId
);

define_id!(
    /// Opaque identifier for a certificate artifact produced by execution or oracles.
    CertificateId
);

define_id!(
    /// Opaque identifier for a bounded oracle slice definition.
    OracleSliceId
);

define_id!(
    /// Opaque identifier for a bounded execution region.
    RegionId
);

define_id!(
    /// Opaque identifier for a stable digest of one bounded execution region.
    RegionDigestId
);

define_id!(
    /// Opaque identifier for a typed artifact transport crossing region boundaries.
    ArtifactTransportId
);

define_id!(
    /// Opaque identifier for syndrome-first repair routing.
    RepairRouteId
);

define_id!(
    /// Opaque identifier for one ranked local repair candidate.
    RepairCandidateId
);

define_id!(
    /// Opaque identifier for a nuisance-state artifact.
    NuisanceStateId
);

define_id!(
    /// Opaque identifier for a convergence-governance artifact.
    ConvergenceReportId
);

define_id!(
    /// Opaque identifier for a registered recursive operator contract.
    OperatorId
);

define_id!(
    /// Opaque identifier for a specific version of a registered operator contract.
    ///
    /// `OperatorId` stays stable across versions; `OperatorVersionId` changes when
    /// the executable contract or policy-relevant operator surface changes.
    OperatorVersionId
);

define_id!(
    /// Opaque identifier for a structured refutation result artifact.
    RefutationResultId
);

define_id!(
    /// Opaque identifier for a structured calibration report artifact.
    CalibrationReportId
);

define_id!(
    /// Opaque identifier for a verification-control case.
    VerificationCaseId
);

define_id!(
    /// Opaque identifier for a canonical verification check plan.
    CheckPlanId
);

define_id!(
    /// Opaque identifier for a canonical control-plane receipt.
    ControlReceiptId
);

define_id!(
    /// Opaque identifier for an append-only verification ledger entry.
    LedgerEntryId
);

define_id!(
    /// Opaque identifier for a policy decision artifact.
    PolicyDecisionId
);

define_id!(
    /// Opaque identifier for a runtime execution permit.
    ///
    /// Minted only after policy, approval, and plan checks succeed for one
    /// concrete execution surface.
    ExecutionPermitId
);

define_id!(
    /// Opaque identifier for an approval record artifact.
    ApprovalRecordId
);

define_id!(
    /// Opaque identifier for a typed constitutional approval grant.
    ///
    /// Grants carry scoped approval lineage for effectful tool dispatch.
    ApprovalGrantId
);

define_id!(
    /// Opaque identifier for a promotion decision artifact.
    PromotionDecisionId
);

define_id!(
    /// Opaque identifier for a refutation decision artifact.
    RefutationDecisionId
);

define_id!(
    /// Opaque identifier for a rollback plan artifact.
    RollbackPlanId
);

define_id!(
    /// Opaque identifier for a calibration snapshot artifact.
    CalibrationSnapshotId
);

define_id!(
    /// Opaque identifier for a versioned learning update artifact.
    LearningUpdateId
);

define_id!(
    /// Opaque identifier for a deterministic boundary repair record artifact.
    BoundaryRepairRecordId
);

define_id!(
    /// Opaque identifier for a semantics profile artifact.
    SemanticsProfileId
);

define_id!(
    /// Opaque identifier for a claim-state artifact.
    ClaimStateId
);

define_id!(
    /// Opaque identifier for a semantic diff artifact.
    SemanticDiffId
);

define_id!(
    /// Opaque identifier for a causal-attribution bundle artifact.
    CausalAttributionBundleId
);

define_id!(
    /// Opaque identifier for an explicit degradation record artifact.
    DegradationRecordId
);

define_id!(
    /// Opaque identifier for an exactness-budget artifact.
    ExactnessBudgetId
);

define_id!(
    /// Opaque identifier for one exported or imported support-set artifact.
    ///
    /// Stable for one snapshot of the support expression attached to a claim.
    SupportSetId
);

define_id!(
    /// Opaque identifier for one contradiction-witness artifact.
    ///
    /// Used to point at a stored explanation of why a claim is simultaneously
    /// supported and refuted within one semantics profile.
    ContradictionWitnessId
);

define_id!(
    /// Opaque identifier for one retraction / supersession artifact.
    ///
    /// This identifies the transaction-time event that closes currentness for
    /// a claim version without erasing historical visibility.
    RetractionRecordId
);

define_id!(
    /// Opaque identifier for an intervention bundle artifact.
    InterventionId
);

define_id!(
    /// Opaque identifier for an outcome-schema artifact.
    OutcomeSchemaId
);

define_id!(
    /// Opaque identifier for an experiment case artifact.
    ExperimentCaseId
);

define_id!(
    /// Opaque identifier for a cohort-contract artifact.
    CohortContractId
);

define_id!(
    /// Opaque identifier for a comparability-matrix artifact.
    ComparabilityMatrixId
);

define_id!(
    /// Opaque identifier for a counterfactual-slice artifact.
    CounterfactualSliceId
);

define_id!(
    /// Opaque identifier for a decision-trace artifact.
    DecisionTraceId
);

define_id!(
    /// Opaque identifier for a refuter-suite artifact.
    RefuterSuiteId
);

define_id!(
    /// Opaque identifier for a refuter-result artifact.
    RefuterResultId
);

define_id!(
    /// Opaque identifier for a rollout-decision artifact.
    RolloutDecisionId
);

define_id!(
    /// Opaque identifier for a rollback-decision artifact.
    RollbackDecisionId
);

define_id!(
    /// Opaque identifier for an experiment-budget artifact.
    ExperimentBudgetId
);

define_id!(
    /// Opaque identifier for an attestation-envelope artifact.
    AttestationEnvelopeId
);

define_id!(
    /// Opaque identifier for a trust-root-set artifact.
    TrustRootSetId
);

define_id!(
    /// Opaque identifier for an artifact-admission-policy artifact.
    ArtifactAdmissionPolicyId
);

define_id!(
    /// Opaque identifier for a transparency-receipt artifact.
    TransparencyReceiptId
);

define_id!(
    /// Opaque identifier for an attestation-revocation artifact.
    AttestationRevocationId
);

define_id!(
    /// Opaque identifier for an attestation-supersession artifact.
    AttestationSupersessionId
);

define_id!(
    /// Opaque identifier for a remote-oracle lease artifact.
    RemoteOracleLeaseId
);

define_id!(
    /// Opaque identifier for a remote-slice request artifact.
    RemoteSliceRequestId
);

define_id!(
    /// Opaque identifier for a remote-slice result artifact.
    RemoteSliceResultId
);

define_id!(
    /// Opaque identifier for a cross-runtime replay-ticket artifact.
    CrossRuntimeReplayTicketId
);

define_id!(
    /// Opaque identifier for a dispute-bundle artifact.
    DisputeBundleId
);

define_id!(
    /// Opaque identifier for a disclosure-policy artifact.
    DisclosurePolicyId
);

define_id!(
    /// Opaque identifier for a disclosure-budget artifact.
    DisclosureBudgetId
);

define_id!(
    /// Opaque identifier for a treaty bundle artifact.
    TreatyBundleId
);

define_id!(
    /// Opaque identifier for a runtime identity set artifact.
    RuntimeIdentitySetId
);

define_id!(
    /// Opaque identifier for a cross-runtime equivalence bundle artifact.
    CrossRuntimeEquivalenceBundleId
);

define_id!(
    /// Opaque identifier for a settlement case artifact.
    SettlementCaseId
);

define_id!(
    /// Opaque identifier for a shared disposition artifact.
    SharedDispositionId
);

define_id!(
    /// Opaque identifier for a local dissent record artifact.
    LocalDissentId
);

define_id!(
    /// Opaque identifier for a shared-view downgrade artifact.
    SharedViewDowngradeId
);

define_id!(
    /// Opaque identifier for a settlement receipt artifact.
    SettlementReceiptId
);

define_id!(
    /// Opaque identifier for a shared replay slice artifact.
    SharedReplaySliceId
);

define_id!(
    /// Opaque identifier for a shared divergence report artifact.
    SharedDivergenceReportId
);

define_id!(
    /// Opaque identifier for a treaty suspension artifact.
    TreatySuspensionId
);

define_id!(
    /// Opaque identifier for a mechanism bundle artifact.
    MechanismBundleId
);

define_id!(
    /// Opaque identifier for a theory version artifact.
    TheoryVersionId
);

define_id!(
    /// Opaque identifier for a theory library artifact.
    TheoryLibraryId
);

define_id!(
    /// Opaque identifier for a hypothesis library artifact.
    HypothesisLibraryId
);

define_id!(
    /// Opaque identifier for a simulation contract artifact.
    SimulationContractId
);

define_id!(
    /// Opaque identifier for a fit-run artifact.
    FitRunId
);

define_id!(
    /// Opaque identifier for a theory refuter suite artifact.
    TheoryRefuterSuiteId
);

define_id!(
    /// Opaque identifier for a rollout stability report artifact.
    RolloutStabilityReportId
);

define_id!(
    /// Opaque identifier for a discovery program artifact.
    DiscoveryProgramId
);

define_id!(
    /// Opaque identifier for a portfolio plan artifact.
    PortfolioPlanId
);

define_id!(
    /// Opaque identifier for an experiment campaign artifact.
    ExperimentCampaignId
);

define_id!(
    /// Opaque identifier for a campaign decision trace artifact.
    CampaignDecisionTraceId
);

define_id!(
    /// Opaque identifier for an information-value estimate artifact.
    InformationValueEstimateId
);

define_id!(
    /// Opaque identifier for a verification-load budget artifact.
    VerificationLoadBudgetId
);

define_id!(
    /// Opaque identifier for a charter bundle artifact.
    CharterBundleId
);

define_id!(
    /// Opaque identifier for a doctrine snapshot artifact.
    DoctrineSnapshotId
);

define_id!(
    /// Opaque identifier for an amendment proposal artifact.
    AmendmentProposalId
);

define_id!(
    /// Opaque identifier for an amendment decision artifact.
    AmendmentDecisionId
);

define_id!(
    /// Opaque identifier for an archive manifest artifact.
    ArchiveManifestId
);

define_id!(
    /// Opaque identifier for a compaction receipt artifact.
    CompactionReceiptId
);

define_id!(
    /// Opaque identifier for a historical query guarantee artifact.
    HistoricalQueryGuaranteeId
);

define_id!(
    /// Opaque identifier for a deprecation bundle artifact.
    DeprecationBundleId
);

define_id!(
    /// Opaque identifier for a retirement bundle artifact.
    RetirementBundleId
);

define_id!(
    /// Opaque identifier for a spec bundle artifact.
    SpecBundleId
);

define_id!(
    /// Opaque identifier for a normative AST artifact.
    NormativeAstId
);

define_id!(
    /// Opaque identifier for a generated schema bundle artifact.
    GeneratedSchemaBundleId
);

define_id!(
    /// Opaque identifier for a generated interpreter bundle artifact.
    GeneratedInterpreterBundleId
);

define_id!(
    /// Opaque identifier for a generated conformance corpus artifact.
    GeneratedConformanceCorpusId
);

define_id!(
    /// Opaque identifier for a generated migration plan artifact.
    GeneratedMigrationPlanId
);

define_id!(
    /// Opaque identifier for a proof-obligation set artifact.
    ProofObligationSetId
);

define_id!(
    /// Opaque identifier for a proof evaluation receipt artifact.
    ProofEvaluationReceiptId
);

define_id!(
    /// Opaque identifier for a human-veto bundle artifact.
    HumanVetoBundleId
);

define_id!(
    /// Opaque identifier for a meta-challenge bundle artifact.
    MetaChallengeBundleId
);

define_id!(
    /// Opaque identifier for a self-hosting build receipt artifact.
    SelfHostingBuildReceiptId
);

define_id!(
    /// Opaque identifier for an effect intent artifact.
    EffectIntentId
);

define_id!(
    /// Opaque identifier for an effect preflight report artifact.
    EffectPreflightReportId
);

define_id!(
    /// Opaque identifier for an effect execution window artifact.
    EffectWindowId
);

define_id!(
    /// Opaque identifier for an effect commit decision artifact.
    EffectCommitDecisionId
);

define_id!(
    /// Opaque identifier for an effect execution receipt artifact.
    EffectExecutionReceiptId
);

define_id!(
    /// Opaque identifier for an effect observation bundle artifact.
    EffectObservationBundleId
);

define_id!(
    /// Opaque identifier for a compensation plan artifact.
    CompensationPlanId
);

define_id!(
    /// Opaque identifier for a compensation execution receipt artifact.
    CompensationExecutionReceiptId
);

define_id!(
    /// Opaque identifier for an external effect ledger entry artifact.
    ExternalEffectLedgerEntryId
);

define_id!(
    /// Opaque identifier for a capability class artifact.
    CapabilityClassId
);

define_id!(
    /// Opaque identifier for an authority lease artifact.
    AuthorityLeaseId
);

define_id!(
    /// Opaque identifier for a delegation bundle artifact.
    DelegationBundleId
);

define_id!(
    /// Opaque identifier for an authority chain artifact.
    AuthorityChainId
);

define_id!(
    /// Opaque identifier for a separation-of-duties policy artifact.
    SeparationOfDutiesPolicyId
);

define_id!(
    /// Opaque identifier for a dual-control approval artifact.
    DualControlApprovalId
);

define_id!(
    /// Opaque identifier for a break-glass grant artifact.
    BreakGlassGrantId
);

define_id!(
    /// Opaque identifier for a delegation revocation artifact.
    DelegationRevocationId
);

define_id!(
    /// Opaque identifier for an acting-on-behalf receipt artifact.
    ActingOnBehalfReceiptId
);

define_id!(
    /// Opaque identifier for a conflict disclosure artifact.
    ConflictDisclosureId
);

define_id!(
    /// Opaque identifier for a deployment profile artifact.
    DeploymentProfileId
);

define_id!(
    /// Opaque identifier for an operating envelope artifact.
    OperatingEnvelopeId
);

define_id!(
    /// Opaque identifier for an assurance case artifact.
    AssuranceCaseId
);

define_id!(
    /// Opaque identifier for a hazard register artifact.
    HazardRegisterId
);

define_id!(
    /// Opaque identifier for a control mapping artifact.
    ControlMappingId
);

define_id!(
    /// Opaque identifier for a residual-risk acceptance artifact.
    ResidualRiskAcceptanceId
);

define_id!(
    /// Opaque identifier for a release-readiness decision artifact.
    ReleaseReadinessDecisionId
);

define_id!(
    /// Opaque identifier for a field monitoring plan artifact.
    FieldMonitoringPlanId
);

define_id!(
    /// Opaque identifier for a certification bundle artifact.
    CertificationBundleId
);

define_id!(
    /// Opaque identifier for a recertification trigger artifact.
    RecertificationTriggerId
);

define_id!(
    /// Opaque identifier for a service-level profile artifact.
    ServiceLevelProfileId
);

define_id!(
    /// Opaque identifier for an error-budget ledger artifact.
    ErrorBudgetLedgerId
);

define_id!(
    /// Opaque identifier for an incident case artifact.
    IncidentCaseId
);

define_id!(
    /// Opaque identifier for a containment decision artifact.
    ContainmentDecisionId
);

define_id!(
    /// Opaque identifier for a forensic freeze artifact.
    ForensicFreezeId
);

define_id!(
    /// Opaque identifier for a recovery plan artifact.
    RecoveryPlanId
);

define_id!(
    /// Opaque identifier for a recovery replay slice artifact.
    RecoveryReplaySliceId
);

define_id!(
    /// Opaque identifier for a bounded continuity exception artifact.
    ContinuityExceptionId
);

define_id!(
    /// Opaque identifier for a postmortem bundle artifact.
    PostmortemBundleId
);

define_id!(
    /// Opaque identifier for a resilience exercise artifact.
    ResilienceExerciseId
);

define_id!(
    /// Opaque identifier for an effect review case artifact.
    EffectReviewCaseId
);

define_id!(
    /// Opaque identifier for an effect block receipt artifact.
    EffectBlockReceiptId
);

define_id!(
    /// Opaque identifier for a delegation review case artifact.
    DelegationReviewCaseId
);

define_id!(
    /// Opaque identifier for a release gate case artifact.
    ReleaseGateCaseId
);

define_id!(
    /// Opaque identifier for a continuity review case artifact.
    ContinuityReviewCaseId
);

define_id!(
    /// Opaque identifier for an effect policy profile artifact.
    EffectPolicyProfileId
);

define_id!(
    /// Opaque identifier for a delegation policy profile artifact.
    DelegationPolicyProfileId
);

define_id!(
    /// Opaque identifier for a release policy profile artifact.
    ReleasePolicyProfileId
);

define_id!(
    /// Opaque identifier for a continuity policy profile artifact.
    ContinuityPolicyProfileId
);

define_id!(
    /// Opaque identifier for an effect adjudication receipt artifact.
    EffectAdjudicationReceiptId
);

define_id!(
    /// Opaque identifier for a release rollback decision artifact.
    ReleaseRollbackDecisionId
);

define_id!(
    /// Opaque identifier for a tool effect dispatch receipt artifact.
    ToolEffectDispatchReceiptId
);

define_id!(
    /// Opaque identifier for a privacy/retention profile artifact.
    PrivacyRetentionProfileId
);

define_id!(
    /// Opaque identifier for a redaction rule-set artifact.
    RedactionRuleSetId
);

define_id!(
    /// Opaque identifier for an access-purpose matrix artifact.
    AccessPurposeMatrixId
);

define_id!(
    /// Opaque identifier for an audit extraction policy artifact.
    AuditExtractionPolicyId
);

define_id!(
    /// Opaque identifier for a residency policy profile artifact.
    ResidencyPolicyProfileId
);

define_id!(
    /// Opaque identifier for a tenant-boundary profile artifact.
    TenantBoundaryProfileId
);

define_id!(
    /// Opaque identifier for a cross-boundary transfer class artifact.
    CrossBoundaryTransferClassId
);

define_id!(
    /// Opaque identifier for a locality exception artifact.
    LocalityExceptionId
);

define_id!(
    /// Opaque identifier for a role catalog artifact.
    RoleCatalogId
);

define_id!(
    /// Opaque identifier for a delegation matrix artifact.
    DelegationMatrixId
);

define_id!(
    /// Opaque identifier for an approval matrix artifact.
    ApprovalMatrixId
);

define_id!(
    /// Opaque identifier for a conflict-class catalog artifact.
    ConflictClassCatalogId
);

define_id!(
    /// Opaque identifier for a regulatory-regime profile artifact.
    RegulatoryRegimeProfileId
);

define_id!(
    /// Opaque identifier for a requirement-to-control map artifact.
    RequirementControlMapId
);

define_id!(
    /// Opaque identifier for an evidence collection plan artifact.
    EvidenceCollectionPlanId
);

define_id!(
    /// Opaque identifier for a recertification schedule artifact.
    RecertificationScheduleId
);

define_id!(
    /// Opaque identifier for a hazard library artifact.
    HazardLibraryId
);

define_id!(
    /// Opaque identifier for a hazard scenario artifact.
    HazardScenarioId
);

define_id!(
    /// Opaque identifier for a monitor catalog artifact.
    MonitorCatalogId
);

define_id!(
    /// Opaque identifier for a mitigation playbook artifact.
    MitigationPlaybookId
);

define_id!(
    /// Opaque identifier for a vendor certification adapter artifact.
    VendorCertificationAdapterId
);

define_id!(
    /// Opaque identifier for a vendor evidence translation artifact.
    VendorEvidenceTranslationId
);

define_id!(
    /// Opaque identifier for a vendor trust-root binding artifact.
    VendorTrustRootBindingId
);

define_id!(
    /// Opaque identifier for a vendor revocation handling artifact.
    VendorRevocationHandlingId
);

define_id!(
    /// Opaque identifier for an incident taxonomy artifact.
    IncidentTaxonomyId
);

define_id!(
    /// Opaque identifier for a severity matrix artifact.
    SeverityMatrixId
);

define_id!(
    /// Opaque identifier for a pager-route profile artifact.
    PagerRouteProfileId
);

define_id!(
    /// Opaque identifier for an escalation-clock policy artifact.
    EscalationClockPolicyId
);

define_id!(
    /// Opaque identifier for an applicability-context artifact.
    ApplicabilityContextId
);

define_id!(
    /// Opaque identifier for a normalized profile-set artifact.
    ProfileSetId
);

define_id!(
    /// Opaque identifier for a composition rule-set artifact.
    CompositionRuleSetId
);

define_id!(
    /// Opaque identifier for a composition receipt artifact.
    CompositionReceiptId
);

define_id!(
    /// Opaque identifier for an effective constitution artifact.
    EffectiveConstitutionId
);

define_id!(
    /// Opaque identifier for a compiled obligation-set artifact.
    CompiledObligationSetId
);

define_id!(
    /// Opaque identifier for a composition conflict-set artifact.
    CompositionConflictSetId
);

define_id!(
    /// Opaque identifier for a profile exception bundle artifact.
    ProfileExceptionBundleId
);

define_id!(
    /// Opaque identifier for a policy impact diff artifact.
    PolicyImpactDiffId
);

#[cfg(test)]
#[path = "ids_tests.rs"]
mod tests;
