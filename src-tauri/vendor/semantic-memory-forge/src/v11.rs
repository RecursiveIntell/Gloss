use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use stack_ids::{
    CausalAttributionBundleId, CertificateId, ClaimId, ClaimStateId, DegradationRecordId,
    ExactnessBudgetId, OracleSliceId, RefutationResultId, ScopeKey, SemanticDiffId,
    SemanticsProfileId, WitnessId,
};

pub const SEMANTICS_PROFILE_V1_SCHEMA: &str = "semantics_profile_v1";
pub const CLAIM_STATE_V1_SCHEMA: &str = "claim_state_v1";
pub const WITNESS_ARTIFACT_V1_SCHEMA: &str = "witness_artifact_v1";
pub const CERTIFICATE_ARTIFACT_V1_SCHEMA: &str = "certificate_artifact_v1";
pub const REFUTATION_ARTIFACT_V1_SCHEMA: &str = "refutation_artifact_v1";
pub const SEMANTIC_DIFF_V1_SCHEMA: &str = "semantic_diff_v1";
pub const ORACLE_SLICE_CONTRACT_V1_SCHEMA: &str = "oracle_slice_contract_v1";
pub const CAUSAL_ATTRIBUTION_BUNDLE_V1_SCHEMA: &str = "causal_attribution_bundle_v1";
pub const DEGRADATION_RECORD_V1_SCHEMA: &str = "degradation_record_v1";
pub const EXACTNESS_BUDGET_V1_SCHEMA: &str = "exactness_budget_v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TruthStateV1 {
    Asserted,
    Supported,
    Refuted,
    Abstained,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DegradationKindV1 {
    ThinExport,
    MissingProof,
    MissingReplay,
    ScopeWithheld,
    EvidenceUnavailable,
    OracleUnavailable,
    BudgetExceeded,
    ExactnessDowngraded,
    AdvisoryOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExactnessLevelV1 {
    Exact,
    Conservative,
    Approximate,
    Heuristic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SemanticViewV1 {
    Canonical,
    Projection,
    Runtime,
    CausalAdvisory,
    Operator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceAdmissibilityV1 {
    Admissible,
    Restricted,
    Inadmissible,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SemanticsProfileV1 {
    pub schema_version: String,
    pub semantics_profile_id: SemanticsProfileId,
    pub profile_name: String,
    pub governing_spec_version: String,
    pub default_view: SemanticViewV1,
    pub allowed_views: Vec<SemanticViewV1>,
    pub truth_state_vocabulary: Vec<TruthStateV1>,
    pub degradation_vocabulary: Vec<DegradationKindV1>,
    pub exactness_vocabulary: Vec<ExactnessLevelV1>,
    pub admissibility_vocabulary: Vec<EvidenceAdmissibilityV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl SemanticsProfileV1 {
    /// Validates the semantics profile contract and required vocabularies.
    pub fn validate(&self) -> Result<(), String> {
        ensure_schema(&self.schema_version, SEMANTICS_PROFILE_V1_SCHEMA)?;
        ensure_non_empty_id(self.semantics_profile_id.as_str(), "semantics_profile_id")?;
        ensure_non_empty(&self.profile_name, "profile_name")?;
        ensure_non_empty(&self.governing_spec_version, "governing_spec_version")?;
        ensure_non_empty_vec(&self.allowed_views, "allowed_views")?;
        ensure_non_empty_vec(&self.truth_state_vocabulary, "truth_state_vocabulary")?;
        ensure_non_empty_vec(&self.degradation_vocabulary, "degradation_vocabulary")?;
        ensure_non_empty_vec(&self.exactness_vocabulary, "exactness_vocabulary")?;
        ensure_non_empty_vec(&self.admissibility_vocabulary, "admissibility_vocabulary")?;
        if !self.allowed_views.contains(&self.default_view) {
            return Err("default_view must be included in allowed_views".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClaimStateV1 {
    pub schema_version: String,
    pub claim_state_id: ClaimStateId,
    pub claim_id: ClaimId,
    pub semantics_profile_id: SemanticsProfileId,
    pub view: SemanticViewV1,
    pub truth_state: TruthStateV1,
    pub exactness: ExactnessLevelV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub degradation: Vec<DegradationKindV1>,
    pub evidence_admissibility: EvidenceAdmissibilityV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proof_obligations_remaining: Vec<String>,
    pub policy_action_allowed: bool,
}

impl ClaimStateV1 {
    /// Validates a v1 claim-state artifact before publication or transport.
    pub fn validate(&self) -> Result<(), String> {
        ensure_schema(&self.schema_version, CLAIM_STATE_V1_SCHEMA)?;
        ensure_non_empty_id(self.claim_state_id.as_str(), "claim_state_id")?;
        ensure_non_empty_id(self.claim_id.as_str(), "claim_id")?;
        ensure_non_empty_id(self.semantics_profile_id.as_str(), "semantics_profile_id")?;
        if self.policy_action_allowed
            && matches!(
                self.truth_state,
                TruthStateV1::Unknown | TruthStateV1::Abstained | TruthStateV1::Refuted
            )
        {
            return Err(
                "policy_action_allowed cannot be true for unknown, abstained, or refuted truth"
                    .into(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WitnessArtifactV1 {
    pub schema_version: String,
    pub witness_id: WitnessId,
    pub semantics_profile_id: SemanticsProfileId,
    pub claim_id: ClaimId,
    pub statement: String,
    pub evidence_refs: Vec<String>,
    pub evidence_admissibility: EvidenceAdmissibilityV1,
    pub exactness: ExactnessLevelV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub degradation: Vec<DegradationKindV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stricter_check_hint: Option<String>,
}

impl WitnessArtifactV1 {
    /// Validates a witness artifact before publication or transport.
    pub fn validate(&self) -> Result<(), String> {
        ensure_schema(&self.schema_version, WITNESS_ARTIFACT_V1_SCHEMA)?;
        ensure_non_empty_id(self.witness_id.as_str(), "witness_id")?;
        ensure_non_empty_id(self.semantics_profile_id.as_str(), "semantics_profile_id")?;
        ensure_non_empty_id(self.claim_id.as_str(), "claim_id")?;
        ensure_non_empty(&self.statement, "statement")?;
        ensure_non_empty_vec(&self.evidence_refs, "evidence_refs")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CertificateKindV1 {
    Execution,
    Replay,
    Consistency,
    ExactnessBound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CertificateArtifactV1 {
    pub schema_version: String,
    pub certificate_id: CertificateId,
    pub semantics_profile_id: SemanticsProfileId,
    pub claim_id: ClaimId,
    pub certificate_kind: CertificateKindV1,
    pub certified_statement: String,
    pub supporting_witness_ids: Vec<WitnessId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oracle_slice_id: Option<OracleSliceId>,
    pub exactness: ExactnessLevelV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub degradation: Vec<DegradationKindV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proof_obligations_remaining: Vec<String>,
}

impl CertificateArtifactV1 {
    /// Validates a certificate artifact before publication or transport.
    pub fn validate(&self) -> Result<(), String> {
        ensure_schema(&self.schema_version, CERTIFICATE_ARTIFACT_V1_SCHEMA)?;
        ensure_non_empty_id(self.certificate_id.as_str(), "certificate_id")?;
        ensure_non_empty_id(self.semantics_profile_id.as_str(), "semantics_profile_id")?;
        ensure_non_empty_id(self.claim_id.as_str(), "claim_id")?;
        ensure_non_empty(&self.certified_statement, "certified_statement")?;
        ensure_non_empty_vec(&self.supporting_witness_ids, "supporting_witness_ids")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RefutationOutcomeV1 {
    Sustained,
    Refuted,
    Inconclusive,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RefutationArtifactV1 {
    pub schema_version: String,
    pub refutation_result_id: RefutationResultId,
    pub semantics_profile_id: SemanticsProfileId,
    pub claim_id: ClaimId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_artifact_id: Option<String>,
    pub refuter: String,
    pub outcome: RefutationOutcomeV1,
    pub exactness: ExactnessLevelV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub degradation: Vec<DegradationKindV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub searched_budget_units: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witness_id: Option<WitnessId>,
    pub reason: String,
}

impl RefutationArtifactV1 {
    /// Validates a refutation artifact before publication or transport.
    pub fn validate(&self) -> Result<(), String> {
        ensure_schema(&self.schema_version, REFUTATION_ARTIFACT_V1_SCHEMA)?;
        ensure_non_empty_id(self.refutation_result_id.as_str(), "refutation_result_id")?;
        ensure_non_empty_id(self.semantics_profile_id.as_str(), "semantics_profile_id")?;
        ensure_non_empty_id(self.claim_id.as_str(), "claim_id")?;
        ensure_non_empty(&self.refuter, "refuter")?;
        ensure_non_empty(&self.reason, "reason")?;
        if matches!(self.outcome, RefutationOutcomeV1::Refuted) && self.witness_id.is_none() {
            return Err("refuted outcomes require a witness_id".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SemanticDiffV1 {
    pub schema_version: String,
    pub semantic_diff_id: SemanticDiffId,
    pub semantics_profile_id: SemanticsProfileId,
    pub subject_kind: String,
    pub subject_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_truth_state: Option<TruthStateV1>,
    pub to_truth_state: TruthStateV1,
    pub changed_fields: Vec<String>,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<String>,
}

impl SemanticDiffV1 {
    /// Validates a semantic-diff artifact before publication or transport.
    pub fn validate(&self) -> Result<(), String> {
        ensure_schema(&self.schema_version, SEMANTIC_DIFF_V1_SCHEMA)?;
        ensure_non_empty_id(self.semantic_diff_id.as_str(), "semantic_diff_id")?;
        ensure_non_empty_id(self.semantics_profile_id.as_str(), "semantics_profile_id")?;
        ensure_non_empty(&self.subject_kind, "subject_kind")?;
        ensure_non_empty(&self.subject_id, "subject_id")?;
        ensure_non_empty_vec(&self.changed_fields, "changed_fields")?;
        ensure_non_empty(&self.reason, "reason")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DegradationActionV1 {
    Block,
    AdvisoryOnly,
    ConservativeFallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OracleSliceContractV1 {
    pub schema_version: String,
    pub slice_id: OracleSliceId,
    pub semantics_profile_id: SemanticsProfileId,
    pub slice_name: String,
    pub scope_key: ScopeKey,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<String>,
    pub exactness: ExactnessLevelV1,
    pub degradation_action: DegradationActionV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rows: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub admissible_regions: Vec<String>,
}

impl OracleSliceContractV1 {
    /// Validates an oracle-slice contract before publication or transport.
    pub fn validate(&self) -> Result<(), String> {
        ensure_schema(&self.schema_version, ORACLE_SLICE_CONTRACT_V1_SCHEMA)?;
        ensure_non_empty_id(self.slice_id.as_str(), "slice_id")?;
        ensure_non_empty_id(self.semantics_profile_id.as_str(), "semantics_profile_id")?;
        ensure_non_empty(&self.slice_name, "slice_name")?;
        if self.max_rows == Some(0) {
            return Err("max_rows must be greater than zero when present".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CausalRoleV1 {
    Treatment,
    Outcome,
    Confounder,
    Mediator,
    Instrument,
    EffectModifier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CausalDirectionV1 {
    Supports,
    Attenuates,
    Refutes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CausalContributorV1 {
    pub factor_id: String,
    pub role: CausalRoleV1,
    pub direction: CausalDirectionV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CausalAttributionBundleV1 {
    pub schema_version: String,
    pub causal_attribution_bundle_id: CausalAttributionBundleId,
    pub semantics_profile_id: SemanticsProfileId,
    pub claim_id: ClaimId,
    pub treatment: String,
    pub outcome: String,
    pub view: SemanticViewV1,
    pub contributors: Vec<CausalContributorV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refutation_result_ids: Vec<RefutationResultId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replay_slice_refs: Vec<String>,
    pub advisory_only: bool,
}

impl CausalAttributionBundleV1 {
    /// Validates an advisory-only causal-attribution bundle before publication or transport.
    pub fn validate(&self) -> Result<(), String> {
        ensure_schema(&self.schema_version, CAUSAL_ATTRIBUTION_BUNDLE_V1_SCHEMA)?;
        ensure_non_empty_id(
            self.causal_attribution_bundle_id.as_str(),
            "causal_attribution_bundle_id",
        )?;
        ensure_non_empty_id(self.semantics_profile_id.as_str(), "semantics_profile_id")?;
        ensure_non_empty_id(self.claim_id.as_str(), "claim_id")?;
        ensure_non_empty(&self.treatment, "treatment")?;
        ensure_non_empty(&self.outcome, "outcome")?;
        ensure_non_empty_vec(&self.contributors, "contributors")?;
        if !self.advisory_only {
            return Err("causal attribution outputs must remain advisory_only".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DegradationRecordV1 {
    pub schema_version: String,
    pub degradation_record_id: DegradationRecordId,
    pub semantics_profile_id: SemanticsProfileId,
    pub artifact_family: String,
    pub artifact_id: String,
    pub degradation: Vec<DegradationKindV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggered_by: Vec<String>,
    pub exactness_impact: ExactnessLevelV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_used: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_action: Option<String>,
}

impl DegradationRecordV1 {
    /// Validates a degradation record before publication or transport.
    pub fn validate(&self) -> Result<(), String> {
        ensure_schema(&self.schema_version, DEGRADATION_RECORD_V1_SCHEMA)?;
        ensure_non_empty_id(self.degradation_record_id.as_str(), "degradation_record_id")?;
        ensure_non_empty_id(self.semantics_profile_id.as_str(), "semantics_profile_id")?;
        ensure_non_empty(&self.artifact_family, "artifact_family")?;
        ensure_non_empty(&self.artifact_id, "artifact_id")?;
        ensure_non_empty_vec(&self.degradation, "degradation")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExactnessEscalationRuleV1 {
    pub when_degraded: DegradationKindV1,
    pub escalate_to: ExactnessLevelV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_action: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExactnessBudgetV1 {
    pub schema_version: String,
    pub exactness_budget_id: ExactnessBudgetId,
    pub semantics_profile_id: SemanticsProfileId,
    pub subject: String,
    pub requested_exactness: ExactnessLevelV1,
    pub achieved_exactness: ExactnessLevelV1,
    pub budget_units: u64,
    pub units_consumed: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub degradation: Vec<DegradationKindV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub escalation_rules: Vec<ExactnessEscalationRuleV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failure_artifact_refs: Vec<String>,
}

impl ExactnessBudgetV1 {
    /// Validates an exactness-budget record before publication or transport.
    pub fn validate(&self) -> Result<(), String> {
        ensure_schema(&self.schema_version, EXACTNESS_BUDGET_V1_SCHEMA)?;
        ensure_non_empty_id(self.exactness_budget_id.as_str(), "exactness_budget_id")?;
        ensure_non_empty_id(self.semantics_profile_id.as_str(), "semantics_profile_id")?;
        ensure_non_empty(&self.subject, "subject")?;
        if self.units_consumed > self.budget_units {
            return Err("units_consumed cannot exceed budget_units".into());
        }
        if self.achieved_exactness != self.requested_exactness && self.degradation.is_empty() {
            return Err("exactness downgrades require degradation markers".into());
        }
        Ok(())
    }
}

fn ensure_schema(actual: &str, expected: &str) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "schema_version must be '{expected}', got '{actual}'"
        ))
    }
}

fn ensure_non_empty(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{field} must not be empty"))
    } else {
        Ok(())
    }
}

fn ensure_non_empty_id(value: &str, field: &str) -> Result<(), String> {
    ensure_non_empty(value, field)
}

fn ensure_non_empty_vec<T>(value: &[T], field: &str) -> Result<(), String> {
    if value.is_empty() {
        Err(format!("{field} must not be empty"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_profile() -> SemanticsProfileV1 {
        SemanticsProfileV1 {
            schema_version: SEMANTICS_PROFILE_V1_SCHEMA.into(),
            semantics_profile_id: SemanticsProfileId::new("semantics-profile-1"),
            profile_name: "canonical-v11".into(),
            governing_spec_version: "v11".into(),
            default_view: SemanticViewV1::Canonical,
            allowed_views: vec![SemanticViewV1::Canonical, SemanticViewV1::Projection],
            truth_state_vocabulary: vec![TruthStateV1::Supported, TruthStateV1::Refuted],
            degradation_vocabulary: vec![
                DegradationKindV1::MissingProof,
                DegradationKindV1::ExactnessDowngraded,
            ],
            exactness_vocabulary: vec![ExactnessLevelV1::Exact, ExactnessLevelV1::Conservative],
            admissibility_vocabulary: vec![
                EvidenceAdmissibilityV1::Admissible,
                EvidenceAdmissibilityV1::Restricted,
            ],
            notes: vec!["proof-bearing outputs only".into()],
        }
    }

    #[test]
    fn semantics_profile_roundtrips_and_validates() {
        let profile = sample_profile();
        profile.validate().unwrap();

        let encoded = serde_json::to_string(&profile).unwrap();
        let decoded: SemanticsProfileV1 = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, profile);
    }

    #[test]
    fn claim_state_roundtrips_and_blocks_unsafe_policy() {
        let claim_state = ClaimStateV1 {
            schema_version: CLAIM_STATE_V1_SCHEMA.into(),
            claim_state_id: ClaimStateId::new("claim-state-1"),
            claim_id: ClaimId::new("claim-1"),
            semantics_profile_id: SemanticsProfileId::new("semantics-profile-1"),
            view: SemanticViewV1::Canonical,
            truth_state: TruthStateV1::Supported,
            exactness: ExactnessLevelV1::Conservative,
            degradation: vec![DegradationKindV1::MissingProof],
            evidence_admissibility: EvidenceAdmissibilityV1::Restricted,
            proof_obligations_remaining: vec!["replay slice missing".into()],
            policy_action_allowed: false,
        };
        claim_state.validate().unwrap();

        let encoded = serde_json::to_string(&claim_state).unwrap();
        let decoded: ClaimStateV1 = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, claim_state);

        let mut invalid = claim_state.clone();
        invalid.truth_state = TruthStateV1::Unknown;
        invalid.policy_action_allowed = true;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn artifact_contracts_roundtrip_and_validate() {
        let witness = WitnessArtifactV1 {
            schema_version: WITNESS_ARTIFACT_V1_SCHEMA.into(),
            witness_id: WitnessId::new("witness-1"),
            semantics_profile_id: SemanticsProfileId::new("semantics-profile-1"),
            claim_id: ClaimId::new("claim-1"),
            statement: "bounded replay reproduced the observed behavior".into(),
            evidence_refs: vec!["receipt:1".into()],
            evidence_admissibility: EvidenceAdmissibilityV1::Admissible,
            exactness: ExactnessLevelV1::Exact,
            degradation: Vec::new(),
            stricter_check_hint: Some("full proof certificate".into()),
        };
        witness.validate().unwrap();
        assert_eq!(
            serde_json::from_str::<WitnessArtifactV1>(&serde_json::to_string(&witness).unwrap())
                .unwrap(),
            witness
        );

        let certificate = CertificateArtifactV1 {
            schema_version: CERTIFICATE_ARTIFACT_V1_SCHEMA.into(),
            certificate_id: CertificateId::new("certificate-1"),
            semantics_profile_id: SemanticsProfileId::new("semantics-profile-1"),
            claim_id: ClaimId::new("claim-1"),
            certificate_kind: CertificateKindV1::Replay,
            certified_statement: "replay remained within bounded semantics".into(),
            supporting_witness_ids: vec![WitnessId::new("witness-1")],
            oracle_slice_id: Some(OracleSliceId::new("oracle-slice-1")),
            exactness: ExactnessLevelV1::Conservative,
            degradation: vec![DegradationKindV1::AdvisoryOnly],
            proof_obligations_remaining: vec!["strict oracle parity".into()],
        };
        certificate.validate().unwrap();
        assert_eq!(
            serde_json::from_str::<CertificateArtifactV1>(
                &serde_json::to_string(&certificate).unwrap()
            )
            .unwrap(),
            certificate
        );

        let refutation = RefutationArtifactV1 {
            schema_version: REFUTATION_ARTIFACT_V1_SCHEMA.into(),
            refutation_result_id: RefutationResultId::new("refutation-1"),
            semantics_profile_id: SemanticsProfileId::new("semantics-profile-1"),
            claim_id: ClaimId::new("claim-1"),
            target_artifact_id: Some("certificate-1".into()),
            refuter: "bounded_kernel_slice".into(),
            outcome: RefutationOutcomeV1::Refuted,
            exactness: ExactnessLevelV1::Conservative,
            degradation: vec![DegradationKindV1::BudgetExceeded],
            searched_budget_units: Some(25),
            witness_id: Some(WitnessId::new("witness-2")),
            reason: "counterexample survives stricter replay".into(),
        };
        refutation.validate().unwrap();
        assert_eq!(
            serde_json::from_str::<RefutationArtifactV1>(
                &serde_json::to_string(&refutation).unwrap()
            )
            .unwrap(),
            refutation
        );

        let diff = SemanticDiffV1 {
            schema_version: SEMANTIC_DIFF_V1_SCHEMA.into(),
            semantic_diff_id: SemanticDiffId::new("semantic-diff-1"),
            semantics_profile_id: SemanticsProfileId::new("semantics-profile-1"),
            subject_kind: "claim".into(),
            subject_id: "claim-1".into(),
            from_truth_state: Some(TruthStateV1::Asserted),
            to_truth_state: TruthStateV1::Supported,
            changed_fields: vec!["truth_state".into(), "proof_obligations_remaining".into()],
            reason: "proof-carrying replay completed".into(),
            evidence_refs: vec!["receipt:1".into()],
        };
        diff.validate().unwrap();
        assert_eq!(
            serde_json::from_str::<SemanticDiffV1>(&serde_json::to_string(&diff).unwrap()).unwrap(),
            diff
        );

        let oracle_slice = OracleSliceContractV1 {
            schema_version: ORACLE_SLICE_CONTRACT_V1_SCHEMA.into(),
            slice_id: OracleSliceId::new("oracle-slice-1"),
            semantics_profile_id: SemanticsProfileId::new("semantics-profile-1"),
            slice_name: "bounded-proof-slice".into(),
            scope_key: ScopeKey::namespace_only("ops"),
            evidence_refs: vec!["receipt:1".into()],
            exactness: ExactnessLevelV1::Conservative,
            degradation_action: DegradationActionV1::AdvisoryOnly,
            max_rows: Some(64),
            admissible_regions: vec!["us-central".into()],
        };
        oracle_slice.validate().unwrap();
        assert_eq!(
            serde_json::from_str::<OracleSliceContractV1>(
                &serde_json::to_string(&oracle_slice).unwrap()
            )
            .unwrap(),
            oracle_slice
        );

        let causal = CausalAttributionBundleV1 {
            schema_version: CAUSAL_ATTRIBUTION_BUNDLE_V1_SCHEMA.into(),
            causal_attribution_bundle_id: CausalAttributionBundleId::new("causal-attribution-1"),
            semantics_profile_id: SemanticsProfileId::new("semantics-profile-1"),
            claim_id: ClaimId::new("claim-1"),
            treatment: "enable bounded replay".into(),
            outcome: "promotion eligibility".into(),
            view: SemanticViewV1::CausalAdvisory,
            contributors: vec![CausalContributorV1 {
                factor_id: "replay-proof".into(),
                role: CausalRoleV1::Treatment,
                direction: CausalDirectionV1::Supports,
                evidence_refs: vec!["receipt:1".into()],
            }],
            refutation_result_ids: vec![RefutationResultId::new("refutation-1")],
            replay_slice_refs: vec!["replay:slice:1".into()],
            advisory_only: true,
        };
        causal.validate().unwrap();
        assert_eq!(
            serde_json::from_str::<CausalAttributionBundleV1>(
                &serde_json::to_string(&causal).unwrap()
            )
            .unwrap(),
            causal
        );

        let degradation = DegradationRecordV1 {
            schema_version: DEGRADATION_RECORD_V1_SCHEMA.into(),
            degradation_record_id: DegradationRecordId::new("degradation-record-1"),
            semantics_profile_id: SemanticsProfileId::new("semantics-profile-1"),
            artifact_family: "certificate_artifact_v1".into(),
            artifact_id: "certificate-1".into(),
            degradation: vec![DegradationKindV1::MissingProof],
            triggered_by: vec!["proof obligation not satisfied".into()],
            exactness_impact: ExactnessLevelV1::Conservative,
            fallback_used: Some("advisory-only output".into()),
            blocked_action: Some("promotion".into()),
        };
        degradation.validate().unwrap();
        assert_eq!(
            serde_json::from_str::<DegradationRecordV1>(
                &serde_json::to_string(&degradation).unwrap()
            )
            .unwrap(),
            degradation
        );

        let budget = ExactnessBudgetV1 {
            schema_version: EXACTNESS_BUDGET_V1_SCHEMA.into(),
            exactness_budget_id: ExactnessBudgetId::new("exactness-budget-1"),
            semantics_profile_id: SemanticsProfileId::new("semantics-profile-1"),
            subject: "claim-1".into(),
            requested_exactness: ExactnessLevelV1::Exact,
            achieved_exactness: ExactnessLevelV1::Conservative,
            budget_units: 100,
            units_consumed: 80,
            degradation: vec![DegradationKindV1::ExactnessDowngraded],
            escalation_rules: vec![ExactnessEscalationRuleV1 {
                when_degraded: DegradationKindV1::ExactnessDowngraded,
                escalate_to: ExactnessLevelV1::Exact,
                block_action: Some("promotion".into()),
            }],
            failure_artifact_refs: vec!["degradation-record-1".into()],
        };
        budget.validate().unwrap();
        assert_eq!(
            serde_json::from_str::<ExactnessBudgetV1>(&serde_json::to_string(&budget).unwrap())
                .unwrap(),
            budget
        );
    }

    #[test]
    fn good_json_fixtures_parse_and_validate() {
        let good_profile = json!({
            "schema_version": "semantics_profile_v1",
            "semantics_profile_id": "semantics-profile-1",
            "profile_name": "canonical-v11",
            "governing_spec_version": "v11",
            "default_view": "canonical",
            "allowed_views": ["canonical", "projection"],
            "truth_state_vocabulary": ["supported", "refuted"],
            "degradation_vocabulary": ["missing_proof", "exactness_downgraded"],
            "exactness_vocabulary": ["exact", "conservative"],
            "admissibility_vocabulary": ["admissible", "restricted"]
        });
        let profile: SemanticsProfileV1 = serde_json::from_value(good_profile).unwrap();
        profile.validate().unwrap();

        let good_claim_state = json!({
            "schema_version": "claim_state_v1",
            "claim_state_id": "claim-state-1",
            "claim_id": "claim-1",
            "semantics_profile_id": "semantics-profile-1",
            "view": "canonical",
            "truth_state": "supported",
            "exactness": "conservative",
            "degradation": ["missing_proof"],
            "evidence_admissibility": "restricted",
            "proof_obligations_remaining": ["replay slice missing"],
            "policy_action_allowed": false
        });
        let claim_state: ClaimStateV1 = serde_json::from_value(good_claim_state).unwrap();
        claim_state.validate().unwrap();

        let good_witness = json!({
            "schema_version": "witness_artifact_v1",
            "witness_id": "witness-1",
            "semantics_profile_id": "semantics-profile-1",
            "claim_id": "claim-1",
            "statement": "bounded replay reproduced the observed behavior",
            "evidence_refs": ["receipt:1"],
            "evidence_admissibility": "admissible",
            "exactness": "exact"
        });
        let witness: WitnessArtifactV1 = serde_json::from_value(good_witness).unwrap();
        witness.validate().unwrap();

        let good_certificate = json!({
            "schema_version": "certificate_artifact_v1",
            "certificate_id": "certificate-1",
            "semantics_profile_id": "semantics-profile-1",
            "claim_id": "claim-1",
            "certificate_kind": "replay",
            "certified_statement": "replay remained within bounded semantics",
            "supporting_witness_ids": ["witness-1"],
            "oracle_slice_id": "oracle-slice-1",
            "exactness": "conservative",
            "degradation": ["advisory_only"],
            "proof_obligations_remaining": ["strict oracle parity"]
        });
        let certificate: CertificateArtifactV1 = serde_json::from_value(good_certificate).unwrap();
        certificate.validate().unwrap();

        let good_refutation = json!({
            "schema_version": "refutation_artifact_v1",
            "refutation_result_id": "refutation-1",
            "semantics_profile_id": "semantics-profile-1",
            "claim_id": "claim-1",
            "target_artifact_id": "certificate-1",
            "refuter": "bounded_kernel_slice",
            "outcome": "refuted",
            "exactness": "conservative",
            "degradation": ["budget_exceeded"],
            "searched_budget_units": 25,
            "witness_id": "witness-2",
            "reason": "counterexample survives stricter replay"
        });
        let refutation: RefutationArtifactV1 = serde_json::from_value(good_refutation).unwrap();
        refutation.validate().unwrap();

        let good_diff = json!({
            "schema_version": "semantic_diff_v1",
            "semantic_diff_id": "semantic-diff-1",
            "semantics_profile_id": "semantics-profile-1",
            "subject_kind": "claim",
            "subject_id": "claim-1",
            "from_truth_state": "asserted",
            "to_truth_state": "supported",
            "changed_fields": ["truth_state", "proof_obligations_remaining"],
            "reason": "proof-carrying replay completed",
            "evidence_refs": ["receipt:1"]
        });
        let diff: SemanticDiffV1 = serde_json::from_value(good_diff).unwrap();
        diff.validate().unwrap();

        let good_oracle_slice = json!({
            "schema_version": "oracle_slice_contract_v1",
            "slice_id": "oracle-slice-1",
            "semantics_profile_id": "semantics-profile-1",
            "slice_name": "bounded-proof-slice",
            "scope_key": { "namespace": "ops" },
            "evidence_refs": ["receipt:1"],
            "exactness": "conservative",
            "degradation_action": "advisory_only",
            "max_rows": 64,
            "admissible_regions": ["us-central"]
        });
        let oracle_slice: OracleSliceContractV1 =
            serde_json::from_value(good_oracle_slice).unwrap();
        oracle_slice.validate().unwrap();

        let good_causal = json!({
            "schema_version": "causal_attribution_bundle_v1",
            "causal_attribution_bundle_id": "causal-attribution-1",
            "semantics_profile_id": "semantics-profile-1",
            "claim_id": "claim-1",
            "treatment": "enable bounded replay",
            "outcome": "promotion eligibility",
            "view": "causal_advisory",
            "contributors": [{
                "factor_id": "replay-proof",
                "role": "treatment",
                "direction": "supports",
                "evidence_refs": ["receipt:1"]
            }],
            "refutation_result_ids": ["refutation-1"],
            "replay_slice_refs": ["replay:slice:1"],
            "advisory_only": true
        });
        let causal: CausalAttributionBundleV1 = serde_json::from_value(good_causal).unwrap();
        causal.validate().unwrap();

        let good_degradation = json!({
            "schema_version": "degradation_record_v1",
            "degradation_record_id": "degradation-record-1",
            "semantics_profile_id": "semantics-profile-1",
            "artifact_family": "certificate_artifact_v1",
            "artifact_id": "certificate-1",
            "degradation": ["missing_proof"],
            "triggered_by": ["proof obligation not satisfied"],
            "exactness_impact": "conservative",
            "fallback_used": "advisory-only output",
            "blocked_action": "promotion"
        });
        let degradation: DegradationRecordV1 = serde_json::from_value(good_degradation).unwrap();
        degradation.validate().unwrap();

        let good_budget = json!({
            "schema_version": "exactness_budget_v1",
            "exactness_budget_id": "exactness-budget-1",
            "semantics_profile_id": "semantics-profile-1",
            "subject": "claim-1",
            "requested_exactness": "exact",
            "achieved_exactness": "conservative",
            "budget_units": 100,
            "units_consumed": 90,
            "degradation": ["exactness_downgraded"],
            "escalation_rules": [{
                "when_degraded": "exactness_downgraded",
                "escalate_to": "exact",
                "block_action": "promotion"
            }]
        });
        let budget: ExactnessBudgetV1 = serde_json::from_value(good_budget).unwrap();
        budget.validate().unwrap();
    }

    #[test]
    fn bad_json_fixtures_are_rejected() {
        let bad_profile = json!({
            "schema_version": "semantics_profile_v1",
            "semantics_profile_id": "semantics-profile-1",
            "profile_name": "",
            "governing_spec_version": "v11",
            "default_view": "runtime",
            "allowed_views": ["canonical"],
            "truth_state_vocabulary": ["supported"],
            "degradation_vocabulary": ["missing_proof"],
            "exactness_vocabulary": ["exact"],
            "admissibility_vocabulary": ["admissible"]
        });
        let profile: SemanticsProfileV1 = serde_json::from_value(bad_profile).unwrap();
        assert!(profile.validate().is_err());

        let bad_claim_state = json!({
            "schema_version": "claim_state_v1",
            "claim_state_id": "claim-state-1",
            "claim_id": "claim-1",
            "semantics_profile_id": "semantics-profile-1",
            "view": "canonical",
            "truth_state": "unknown",
            "exactness": "conservative",
            "evidence_admissibility": "unknown",
            "policy_action_allowed": true
        });
        let claim_state: ClaimStateV1 = serde_json::from_value(bad_claim_state).unwrap();
        assert!(claim_state.validate().is_err());

        let bad_witness = json!({
            "schema_version": "witness_artifact_v1",
            "witness_id": "witness-1",
            "semantics_profile_id": "semantics-profile-1",
            "claim_id": "claim-1",
            "statement": "",
            "evidence_refs": [],
            "evidence_admissibility": "admissible",
            "exactness": "exact"
        });
        let witness: WitnessArtifactV1 = serde_json::from_value(bad_witness).unwrap();
        assert!(witness.validate().is_err());

        let bad_certificate = json!({
            "schema_version": "certificate_artifact_v1",
            "certificate_id": "certificate-1",
            "semantics_profile_id": "semantics-profile-1",
            "claim_id": "claim-1",
            "certificate_kind": "replay",
            "certified_statement": "",
            "supporting_witness_ids": [],
            "exactness": "conservative"
        });
        let certificate: CertificateArtifactV1 = serde_json::from_value(bad_certificate).unwrap();
        assert!(certificate.validate().is_err());

        let bad_refutation = json!({
            "schema_version": "refutation_artifact_v1",
            "refutation_result_id": "refutation-1",
            "semantics_profile_id": "semantics-profile-1",
            "claim_id": "claim-1",
            "refuter": "bounded_kernel_slice",
            "outcome": "refuted",
            "exactness": "conservative",
            "reason": "counterexample found"
        });
        let refutation: RefutationArtifactV1 = serde_json::from_value(bad_refutation).unwrap();
        assert!(refutation.validate().is_err());

        let bad_diff = json!({
            "schema_version": "semantic_diff_v1",
            "semantic_diff_id": "semantic-diff-1",
            "semantics_profile_id": "semantics-profile-1",
            "subject_kind": "claim",
            "subject_id": "claim-1",
            "to_truth_state": "supported",
            "changed_fields": [],
            "reason": ""
        });
        let diff: SemanticDiffV1 = serde_json::from_value(bad_diff).unwrap();
        assert!(diff.validate().is_err());

        let bad_oracle_slice = json!({
            "schema_version": "oracle_slice_contract_v1",
            "slice_id": "oracle-slice-1",
            "semantics_profile_id": "semantics-profile-1",
            "slice_name": "",
            "scope_key": { "namespace": "ops" },
            "exactness": "conservative",
            "degradation_action": "advisory_only",
            "max_rows": 0
        });
        let oracle_slice: OracleSliceContractV1 = serde_json::from_value(bad_oracle_slice).unwrap();
        assert!(oracle_slice.validate().is_err());

        let bad_causal = json!({
            "schema_version": "causal_attribution_bundle_v1",
            "causal_attribution_bundle_id": "causal-attribution-1",
            "semantics_profile_id": "semantics-profile-1",
            "claim_id": "claim-1",
            "treatment": "enable bounded replay",
            "outcome": "promotion eligibility",
            "view": "causal_advisory",
            "contributors": [],
            "advisory_only": false
        });
        let causal: CausalAttributionBundleV1 = serde_json::from_value(bad_causal).unwrap();
        assert!(causal.validate().is_err());

        let bad_degradation = json!({
            "schema_version": "degradation_record_v1",
            "degradation_record_id": "degradation-record-1",
            "semantics_profile_id": "semantics-profile-1",
            "artifact_family": "certificate_artifact_v1",
            "artifact_id": "certificate-1",
            "degradation": [],
            "exactness_impact": "conservative"
        });
        let degradation: DegradationRecordV1 = serde_json::from_value(bad_degradation).unwrap();
        assert!(degradation.validate().is_err());

        let bad_budget = json!({
            "schema_version": "exactness_budget_v1",
            "exactness_budget_id": "exactness-budget-1",
            "semantics_profile_id": "semantics-profile-1",
            "subject": "claim-1",
            "requested_exactness": "exact",
            "achieved_exactness": "conservative",
            "budget_units": 10,
            "units_consumed": 11,
            "degradation": []
        });
        let budget: ExactnessBudgetV1 = serde_json::from_value(bad_budget).unwrap();
        assert!(budget.validate().is_err());
    }
}
