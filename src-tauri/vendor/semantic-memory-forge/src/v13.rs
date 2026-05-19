use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use stack_ids::{
    ClaimId, ClaimStateId, ClaimVersionId, ContentDigest, ContradictionWitnessId,
    RetractionRecordId, SemanticsProfileId, SupportSetId,
};

use crate::{DegradationKindV1, EvidenceAdmissibilityV1, ExactnessLevelV1, SemanticViewV1};

pub const BILATTICE_TRUTH_V1_SCHEMA: &str = "bilattice_truth_v1";
pub const SUPPORT_SET_V1_SCHEMA: &str = "support_set_v1";
pub const CONTRADICTION_WITNESS_V1_SCHEMA: &str = "contradiction_witness_v1";
pub const RETRACTION_RECORD_V1_SCHEMA: &str = "retraction_record_v1";
pub const CLAIM_STATE_V13_SCHEMA: &str = "claim_state_v13";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BilatticeTruthV1 {
    Unknown,
    TrueOnly,
    FalseOnly,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SupportPolarityV1 {
    Supports,
    Refutes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SupportProvenanceKindV1 {
    EvidenceRef,
    ClaimVersion,
    RelationVersion,
    Episode,
    Receipt,
    External,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SupportTokenV1 {
    pub token_id: String,
    pub kind: SupportProvenanceKindV1,
    pub reference: String,
    pub polarity: SupportPolarityV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SupportExprV1 {
    Token { token_id: String },
    AnyOf { children: Vec<SupportExprV1> },
    AllOf { children: Vec<SupportExprV1> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SupportSetV1 {
    pub schema_version: String,
    pub support_set_id: SupportSetId,
    pub claim_id: ClaimId,
    pub semantics_profile_id: SemanticsProfileId,
    pub support_tokens: Vec<SupportTokenV1>,
    pub support_expr: SupportExprV1,
    pub content_digest: ContentDigest,
}

impl SupportSetV1 {
    /// Validates a support set and its support expression against the token set.
    pub fn validate(&self) -> Result<(), String> {
        ensure_schema(&self.schema_version, SUPPORT_SET_V1_SCHEMA)?;
        ensure_non_empty_id(self.support_set_id.as_str(), "support_set_id")?;
        ensure_non_empty_id(self.claim_id.as_str(), "claim_id")?;
        ensure_non_empty_id(self.semantics_profile_id.as_str(), "semantics_profile_id")?;
        ensure_non_empty_vec(&self.support_tokens, "support_tokens")?;
        validate_support_expr(&self.support_expr, &self.support_tokens)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QualityVectorV1 {
    pub exactness: ExactnessLevelV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub degradation: Vec<DegradationKindV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freshness: Option<String>,
    pub replay_limited: bool,
    pub execution_contaminated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContradictionWitnessV1 {
    pub schema_version: String,
    pub contradiction_witness_id: ContradictionWitnessId,
    pub claim_id: ClaimId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicting_token_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

impl ContradictionWitnessV1 {
    /// Validates a contradiction witness before publication or transport.
    pub fn validate(&self) -> Result<(), String> {
        ensure_schema(&self.schema_version, CONTRADICTION_WITNESS_V1_SCHEMA)?;
        ensure_non_empty_id(
            self.contradiction_witness_id.as_str(),
            "contradiction_witness_id",
        )?;
        ensure_non_empty_id(self.claim_id.as_str(), "claim_id")?;
        ensure_non_empty_vec(&self.conflicting_token_ids, "conflicting_token_ids")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RetractionRecordV1 {
    pub schema_version: String,
    pub retraction_record_id: RetractionRecordId,
    pub claim_id: ClaimId,
    pub retracted_claim_version_id: ClaimVersionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by_claim_version_id: Option<ClaimVersionId>,
    pub effective_recorded_at: String,
    pub reason: String,
    pub cascade_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta_summary: Option<String>,
}

impl RetractionRecordV1 {
    /// Validates a retraction record before publication or transport.
    pub fn validate(&self) -> Result<(), String> {
        ensure_schema(&self.schema_version, RETRACTION_RECORD_V1_SCHEMA)?;
        ensure_non_empty_id(self.retraction_record_id.as_str(), "retraction_record_id")?;
        ensure_non_empty_id(self.claim_id.as_str(), "claim_id")?;
        ensure_non_empty_id(
            self.retracted_claim_version_id.as_str(),
            "retracted_claim_version_id",
        )?;
        ensure_non_empty(&self.effective_recorded_at, "effective_recorded_at")?;
        ensure_non_empty(&self.reason, "reason")?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClaimStateV13 {
    pub schema_version: String,
    pub claim_state_id: ClaimStateId,
    pub claim_id: ClaimId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_version_id: Option<ClaimVersionId>,
    pub semantics_profile_id: SemanticsProfileId,
    pub view: SemanticViewV1,
    pub bilattice_truth: BilatticeTruthV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support_set_id: Option<SupportSetId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support_set_digest: Option<ContentDigest>,
    pub quality_vector: QualityVectorV1,
    pub evidence_admissibility: EvidenceAdmissibilityV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contradiction_witness_id: Option<ContradictionWitnessId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<String>,
    pub tx_from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_to: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proof_obligations_remaining: Vec<String>,
    pub policy_action_allowed: bool,
}

impl ClaimStateV13 {
    /// Validates a v13 claim-state artifact before publication or transport.
    pub fn validate(&self) -> Result<(), String> {
        ensure_schema(&self.schema_version, CLAIM_STATE_V13_SCHEMA)?;
        ensure_non_empty_id(self.claim_state_id.as_str(), "claim_state_id")?;
        ensure_non_empty_id(self.claim_id.as_str(), "claim_id")?;
        ensure_non_empty_id(self.semantics_profile_id.as_str(), "semantics_profile_id")?;
        ensure_non_empty(&self.tx_from, "tx_from")?;
        if self.support_set_id.is_some() ^ self.support_set_digest.is_some() {
            return Err(
                "support_set_id and support_set_digest must either both be present or both be absent"
                    .into(),
            );
        }
        if self.policy_action_allowed
            && matches!(
                self.bilattice_truth,
                BilatticeTruthV1::Unknown | BilatticeTruthV1::FalseOnly | BilatticeTruthV1::Both
            )
        {
            return Err(
                "policy_action_allowed cannot be true for unknown, false_only, or both truth states"
                    .into(),
            );
        }
        Ok(())
    }
}

fn ensure_schema(actual: &str, expected: &str) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "schema_version mismatch: expected {expected}, got {actual}"
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

fn validate_support_expr(
    expr: &SupportExprV1,
    support_tokens: &[SupportTokenV1],
) -> Result<(), String> {
    match expr {
        SupportExprV1::Token { token_id } => {
            if support_tokens
                .iter()
                .any(|token| token.token_id == *token_id)
            {
                Ok(())
            } else {
                Err(format!(
                    "support_expr references unknown token_id '{token_id}'"
                ))
            }
        }
        SupportExprV1::AnyOf { children } | SupportExprV1::AllOf { children } => {
            ensure_non_empty_vec(children, "support_expr.children")?;
            for child in children {
                validate_support_expr(child, support_tokens)?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_set_requires_known_tokens() {
        let support = SupportSetV1 {
            schema_version: SUPPORT_SET_V1_SCHEMA.into(),
            support_set_id: SupportSetId::new("support-1"),
            claim_id: ClaimId::new("claim-1"),
            semantics_profile_id: SemanticsProfileId::new("profile-1"),
            support_tokens: vec![SupportTokenV1 {
                token_id: "tok-1".into(),
                kind: SupportProvenanceKindV1::EvidenceRef,
                reference: "evidence:1".into(),
                polarity: SupportPolarityV1::Supports,
            }],
            support_expr: SupportExprV1::Token {
                token_id: "tok-1".into(),
            },
            content_digest: ContentDigest::compute(b"support-set"),
        };

        assert!(support.validate().is_ok());
    }

    #[test]
    fn claim_state_v13_rejects_half_present_support_digest_pair() {
        let claim_state = ClaimStateV13 {
            schema_version: CLAIM_STATE_V13_SCHEMA.into(),
            claim_state_id: ClaimStateId::new("claim-state-1"),
            claim_id: ClaimId::new("claim-1"),
            claim_version_id: Some(ClaimVersionId::new("claim-version-1")),
            semantics_profile_id: SemanticsProfileId::new("profile-1"),
            view: SemanticViewV1::Canonical,
            bilattice_truth: BilatticeTruthV1::TrueOnly,
            support_set_id: Some(SupportSetId::new("support-1")),
            support_set_digest: None,
            quality_vector: QualityVectorV1 {
                exactness: ExactnessLevelV1::Conservative,
                degradation: vec![DegradationKindV1::ExactnessDowngraded],
                freshness: Some("current".into()),
                replay_limited: false,
                execution_contaminated: false,
            },
            evidence_admissibility: EvidenceAdmissibilityV1::Admissible,
            contradiction_witness_id: None,
            valid_from: None,
            valid_to: None,
            tx_from: "2026-03-14T12:05:00Z".into(),
            tx_to: None,
            proof_obligations_remaining: vec![],
            policy_action_allowed: true,
        };

        assert!(claim_state.validate().is_err());
    }
}
