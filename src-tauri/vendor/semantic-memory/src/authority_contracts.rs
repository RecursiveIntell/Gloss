//! Versioned wire contracts for authority, witnessed retrieval, and injection governance.

use crate::StateView;
use serde::{Deserialize, Deserializer, Serialize};

macro_rules! bounded_f64 {
    ($name:ident, $min:expr, $max:expr) => {
        #[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(f64);

        impl $name {
            pub fn new(value: f64) -> Result<Self, String> {
                if !value.is_finite() || value < $min || value > $max {
                    return Err(format!(
                        "{} must be finite and within [{}, {}]",
                        stringify!($name),
                        $min,
                        $max
                    ));
                }
                Ok(Self(value))
            }

            pub fn get(self) -> f64 {
                self.0
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = f64::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

bounded_f64!(Probability, 0.0, 1.0);
bounded_f64!(Confidence, 0.0, 1.0);
bounded_f64!(CosineSimilarity, -1.0, 1.0);
bounded_f64!(NonNegativeWeight, 0.0, f64::MAX);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuthoritySnapshotId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RetrievalEpoch(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryEnvelopeV1 {
    pub schema_version: String,
    pub memory_id: String,
    pub namespace: String,
    pub content: String,
    pub source: Option<String>,
    pub valid_at: String,
    pub recorded_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityManifestV1 {
    pub schema_version: String,
    pub principal: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageOutcomeV1 {
    NotPlanned,
    Skipped,
    AnalysisOnly,
    Applied,
    Degraded,
    Failed,
    BudgetExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetrievalWitnessV1 {
    pub schema_version: String,
    pub request_id: String,
    pub evaluated_at: String,
    pub authority_snapshot_id: AuthoritySnapshotId,
    pub retrieval_epoch: RetrievalEpoch,
    pub query_digest: String,
    pub config_digest: String,
    pub ordered_result_ids: Vec<String>,
    pub ordered_result_digests: Vec<String>,
    pub stage_outcomes: Vec<(String, StageOutcomeV1)>,
    pub degradations: Vec<String>,
    pub cached_witness_parent: Option<String>,
}

/// Snapshot metadata used by governing systems for cache validation and replay integrity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityStateV1 {
    pub snapshot_id: AuthoritySnapshotId,
    pub retrieval_epoch: RetrievalEpoch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetrievalResponseV1<T> {
    pub schema_version: String,
    pub state_view: StateView,
    pub results: Vec<T>,
    pub witness: RetrievalWitnessV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectionDisposition {
    Admitted,
    PartiallyAdmitted,
    Rejected,
    FailedClosed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InjectionDecisionV1 {
    pub schema_version: String,
    pub retrieval_receipt_id: String,
    pub principal: String,
    pub host: String,
    pub policy_digest: String,
    pub admitted_ids: Vec<String>,
    pub rejected_ids: Vec<String>,
    pub disposition: InjectionDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupersessionReceiptV1 {
    pub schema_version: String,
    pub operation_id: String,
    pub caller_idempotency_key: String,
    pub superseded_id: String,
    pub replacement_id: String,
    pub authority_snapshot_id: AuthoritySnapshotId,
    pub retrieval_epoch: RetrievalEpoch,
    pub committed_at: String,
}

/// Capability-bearing caller permit for the governed authority mutation lane.
///
/// Ordinary callers cannot construct or deserialize this credential:
/// ```compile_fail
/// use semantic_memory::{AuthorityAdmission, AuthorityPermit};
/// let _forged = AuthorityPermit {
///     principal: "attacker".into(),
///     caller_id: "attacker".into(),
///     capability: AuthorityPermit::APPEND_CAPABILITY.into(),
///     admission: AuthorityAdmission::OperatorSystem,
///     origin_authority: None,
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuthorityPermit {
    pub(crate) principal: String,
    pub(crate) caller_id: String,
    pub(crate) capability: String,
    pub(crate) admission: AuthorityAdmission,
    /// Immutable origin proposed for the governed write. Missing origin fails closed.
    #[serde(default)]
    pub(crate) origin_authority: Option<crate::OriginAuthorityLabelV1>,
}

/// Admission basis carried by a governed authority permit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityAdmission {
    Unspecified,
    Evidence { evidence_refs: Vec<String> },
    OperatorSystem,
}

/// Trusted in-process issuer. It has no public constructor, so ordinary callers cannot mint
/// capability-bearing permits or supply caller identities as request data.
pub struct AuthorityIssuer {
    _private: (),
}

/// Resolver-produced immutable evidence identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEvidenceDigest(String);

impl AuthorityIssuer {
    #[cfg(feature = "testing")]
    pub(crate) fn trusted() -> Self {
        Self { _private: () }
    }

    pub fn mint_operator_system(
        &self,
        principal: impl Into<String>,
        caller_id: impl Into<String>,
        capability: impl Into<String>,
    ) -> AuthorityPermit {
        let principal = principal.into();
        let caller_id = caller_id.into();
        AuthorityPermit {
            origin_authority: Some(crate::OriginAuthorityLabelV1::operator_system(
                &principal, &caller_id,
            )),
            principal,
            caller_id,
            capability: capability.into(),
            admission: AuthorityAdmission::OperatorSystem,
        }
    }

    pub fn mint_with_resolved_evidence(
        &self,
        principal: impl Into<String>,
        caller_id: impl Into<String>,
        capability: impl Into<String>,
        evidence: Vec<ResolvedEvidenceDigest>,
        origin_authority: crate::OriginAuthorityLabelV1,
    ) -> AuthorityPermit {
        AuthorityPermit {
            principal: principal.into(),
            caller_id: caller_id.into(),
            capability: capability.into(),
            admission: AuthorityAdmission::Evidence {
                evidence_refs: evidence.into_iter().map(|digest| digest.0).collect(),
            },
            origin_authority: Some(origin_authority),
        }
    }
}

impl ResolvedEvidenceDigest {
    pub fn from_resolver(digest: String) -> Option<Self> {
        let hex = digest.strip_prefix("blake3:")?;
        (hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then_some(Self(digest))
    }
}

impl AuthorityPermit {
    pub const APPEND_CAPABILITY: &'static str = "memory.authority.append";
    pub const SUPERSEDE_CAPABILITY: &'static str = "memory.authority.supersede";
    pub const REDACT_CAPABILITY: &'static str = "memory.authority.redact";
    pub const REVOKE_ORIGIN_CAPABILITY: &'static str = "memory.authority.revoke_origin";
    pub const FORGET_CAPABILITY: &'static str = "memory.authority.forget";

    #[cfg(feature = "testing")]
    pub fn new(
        principal: impl Into<String>,
        caller_id: impl Into<String>,
        capability: impl Into<String>,
    ) -> Self {
        Self {
            principal: principal.into(),
            caller_id: caller_id.into(),
            capability: capability.into(),
            admission: AuthorityAdmission::Unspecified,
            origin_authority: None,
        }
    }

    /// Construct a permit for a fact proposal supported by explicit evidence.
    #[cfg(feature = "testing")]
    pub fn with_evidence(
        principal: impl Into<String>,
        caller_id: impl Into<String>,
        capability: impl Into<String>,
        evidence_refs: Vec<String>,
    ) -> Self {
        Self {
            principal: principal.into(),
            caller_id: caller_id.into(),
            capability: capability.into(),
            admission: AuthorityAdmission::Evidence { evidence_refs },
            origin_authority: None,
        }
    }

    /// Construct the explicit operator/system bypass used for trusted inserts.
    #[cfg(feature = "testing")]
    pub fn operator_system(
        principal: impl Into<String>,
        caller_id: impl Into<String>,
        capability: impl Into<String>,
    ) -> Self {
        AuthorityIssuer::trusted().mint_operator_system(principal, caller_id, capability)
    }

    /// Bind an immutable origin label to this governed write permit.
    #[cfg(feature = "testing")]
    pub fn with_origin(mut self, origin: crate::OriginAuthorityLabelV1) -> Self {
        self.origin_authority = Some(origin);
        self
    }
}

/// Authority mutation kind recorded in the operation journal and receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityOperationKind {
    Append,
    Supersede,
    Redact,
}

/// Typed fault-injection points for atomic authority tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityFaultStage {
    BeforeAppend,
    AfterAppend,
    BeforeLineage,
    AfterLineage,
    BeforeJournal,
    AfterJournal,
    BeforeEpoch,
    AfterEpoch,
    BeforeReceipt,
    AfterReceipt,
    BeforeForgettingMutation,
    AfterForgettingMutation,
    BeforeForgettingReceipt,
    AfterForgettingReceipt,
    BeforeShadowPromotion,
    AfterShadowPromotion,
}

/// Durable receipt for one governed authority mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityReceiptV1 {
    pub schema_version: String,
    pub receipt_id: String,
    pub operation_id: String,
    pub caller_idempotency_key: String,
    pub principal: String,
    pub caller_id: String,
    pub operation_kind: AuthorityOperationKind,
    pub before_snapshot_id: AuthoritySnapshotId,
    pub after_snapshot_id: AuthoritySnapshotId,
    pub before_epoch: RetrievalEpoch,
    pub after_epoch: RetrievalEpoch,
    pub affected_ids: Vec<String>,
    pub content_digest: String,
    /// Digest of the immutable origin label persisted for the written fact.
    #[serde(default)]
    pub origin_label_digest: Option<String>,
    pub receipt_digest: String,
    pub committed_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_numbers_reject_non_finite_and_out_of_range() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1, 1.1] {
            assert!(Probability::new(value).is_err());
            assert!(serde_json::from_str::<Probability>(&value.to_string()).is_err());
        }
        assert!(CosineSimilarity::new(-1.0).is_ok());
        assert!(CosineSimilarity::new(1.0).is_ok());
        assert!(NonNegativeWeight::new(-f64::EPSILON).is_err());
    }

    #[test]
    fn bounded_number_serde_round_trips_boundaries() {
        for value in [0.0, 0.5, 1.0] {
            let bounded = Probability::new(value).unwrap();
            let json = serde_json::to_string(&bounded).unwrap();
            assert_eq!(serde_json::from_str::<Probability>(&json).unwrap(), bounded);
        }
    }
}
