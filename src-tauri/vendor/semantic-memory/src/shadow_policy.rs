//! Shadow-learned policy proposals with deterministic promotion and rollback.
//!
//! This module is deliberately separate from facts, authority lineages, and active runtime
//! configuration. A proposal is an observation about a possible policy change. It is never a
//! canonical memory write, and the only path that can make it active is the deterministic gate
//! below. The SQLite proposal, version, and receipt tables are append-only audit metadata.

use crate::authority_contracts::AuthorityFaultStage;
use crate::db::with_transaction;
use crate::{MemoryError, MemoryStore};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SHADOW_POLICY_PROPOSAL_V1: &str = "shadow_policy_proposal_v1";
pub const PROMOTION_DECISION_RECEIPT_V1: &str = "promotion_decision_receipt_v1";
const SHADOW_PROPOSAL_MAX_RISK: f64 = 0.75;
const SHADOW_MAX_DELTA: f64 = 1.0;

/// Compute the canonical digest used for a policy JSON value.
pub fn shadow_policy_digest(value: &Value) -> String {
    digest_json(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadowPolicyKindV1 {
    Routing,
    WriteAdmission,
    Retention,
    RerankWeights,
}

impl ShadowPolicyKindV1 {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Routing => "routing",
            Self::WriteAdmission => "write_admission",
            Self::Retention => "retention",
            Self::RerankWeights => "rerank_weights",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadowPolicyStatusV1 {
    Proposed,
    Promoted,
    Rejected,
    Quarantined,
    Deferred,
    Expired,
    RolledBack,
}

impl ShadowPolicyStatusV1 {
    fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Promoted => "promoted",
            Self::Rejected => "rejected",
            Self::Quarantined => "quarantined",
            Self::Deferred => "deferred",
            Self::Expired => "expired",
            Self::RolledBack => "rolled_back",
        }
    }

    fn parse(value: &str) -> Result<Self, MemoryError> {
        match value {
            "proposed" => Ok(Self::Proposed),
            "promoted" => Ok(Self::Promoted),
            "rejected" => Ok(Self::Rejected),
            "quarantined" => Ok(Self::Quarantined),
            "deferred" => Ok(Self::Deferred),
            "expired" => Ok(Self::Expired),
            "rolled_back" => Ok(Self::RolledBack),
            other => Err(MemoryError::CorruptData {
                table: "shadow_policy_proposals",
                row_id: value.to_string(),
                detail: format!("unknown proposal status '{other}'"),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowPolicyProvenanceV1 {
    pub origin: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
}

impl ShadowPolicyProvenanceV1 {
    pub fn new(origin: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            origin: origin.into(),
            source: source.into(),
            model_id: None,
        }
    }

    pub fn with_model(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = Some(model_id.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowEvaluationWindowV1 {
    pub training_start: String,
    pub training_end: String,
    pub evaluation_start: String,
    pub evaluation_end: String,
    pub held_out_input_digest: String,
}

impl ShadowEvaluationWindowV1 {
    pub fn new(
        training_start: impl Into<String>,
        training_end: impl Into<String>,
        evaluation_start: impl Into<String>,
        evaluation_end: impl Into<String>,
        held_out_input_digest: impl Into<String>,
    ) -> Self {
        Self {
            training_start: training_start.into(),
            training_end: training_end.into(),
            evaluation_start: evaluation_start.into(),
            evaluation_end: evaluation_end.into(),
            held_out_input_digest: held_out_input_digest.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShadowPolicyRiskV1 {
    pub score: f64,
    pub categories: Vec<String>,
}

impl ShadowPolicyRiskV1 {
    pub fn new(score: f64, categories: Vec<impl Into<String>>) -> Self {
        Self {
            score,
            categories: categories.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShadowPolicyProposalV1 {
    pub schema_version: String,
    pub proposal_id: String,
    pub idempotency_key: String,
    pub principal: String,
    pub policy_kind: ShadowPolicyKindV1,
    pub provenance: ShadowPolicyProvenanceV1,
    pub training_window: ShadowEvaluationWindowV1,
    pub feature_digest: String,
    pub baseline_policy: Value,
    pub baseline_policy_digest: String,
    pub proposed_delta: Value,
    pub risk: ShadowPolicyRiskV1,
    pub expires_at: String,
    pub status: ShadowPolicyStatusV1,
    pub proposal_digest: String,
    pub created_at: String,
}

impl ShadowPolicyProposalV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        policy_kind: ShadowPolicyKindV1,
        principal: impl Into<String>,
        provenance: ShadowPolicyProvenanceV1,
        training_window: ShadowEvaluationWindowV1,
        feature_digest: impl Into<String>,
        baseline_policy: Value,
        proposed_delta: Value,
        risk: ShadowPolicyRiskV1,
        expires_at: impl Into<String>,
        idempotency_key: impl Into<String>,
    ) -> Self {
        let principal = principal.into();
        let idempotency_key = idempotency_key.into();
        let feature_digest = feature_digest.into();
        let expires_at = expires_at.into();
        let baseline_policy_digest = digest_json(&baseline_policy);
        let created_at = Utc::now().to_rfc3339();
        let proposal_id = format!(
            "shadow:{}",
            digest_json(&serde_json::json!({
                "idempotency_key": idempotency_key.clone(),
                "principal": principal.clone(),
                "policy_kind": policy_kind,
                "provenance": provenance.clone(),
                "training_window": training_window.clone(),
                "feature_digest": feature_digest.clone(),
                "baseline_policy": baseline_policy.clone(),
                "proposed_delta": proposed_delta.clone(),
                "risk": risk.clone(),
                "expires_at": expires_at.clone(),
            }))
        );
        let mut proposal = Self {
            schema_version: SHADOW_POLICY_PROPOSAL_V1.into(),
            proposal_id,
            idempotency_key,
            principal,
            policy_kind,
            provenance,
            training_window,
            feature_digest,
            baseline_policy,
            baseline_policy_digest,
            proposed_delta,
            risk,
            expires_at,
            status: ShadowPolicyStatusV1::Proposed,
            proposal_digest: String::new(),
            created_at,
        };
        proposal.proposal_digest = proposal.compute_digest();
        proposal
    }

    pub fn compute_digest(&self) -> String {
        digest_json(&serde_json::json!({
            "schema_version": self.schema_version,
            "proposal_id": self.proposal_id,
            "idempotency_key": self.idempotency_key,
            "principal": self.principal,
            "policy_kind": self.policy_kind,
            "provenance": self.provenance,
            "training_window": self.training_window,
            "feature_digest": self.feature_digest,
            "baseline_policy": self.baseline_policy,
            "baseline_policy_digest": self.baseline_policy_digest,
            "proposed_delta": self.proposed_delta,
            "risk": self.risk,
            "expires_at": self.expires_at,
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotionEvidenceV1 {
    pub schema_version: String,
    pub proposal_id: String,
    pub held_out_improvement: f64,
    pub safety_regression: bool,
    pub integrity_regression: bool,
    pub metrics: Value,
    pub evaluation_input_hashes: Vec<String>,
    pub metrics_digest: String,
    pub reproducibility_digest: String,
    pub rollback_target: String,
    pub evaluated_at: String,
}

impl PromotionEvidenceV1 {
    pub fn new(
        proposal: &ShadowPolicyProposalV1,
        held_out_improvement: f64,
        safety_regression: bool,
        integrity_regression: bool,
        metrics: Value,
        evaluation_input_hashes: Vec<String>,
        rollback_target: impl Into<String>,
    ) -> Self {
        let metrics_digest = digest_json(&metrics);
        let reproducibility_digest = digest_json(&serde_json::json!({
            "proposal": proposal.proposal_digest,
            "held_out_improvement": held_out_improvement,
            "safety_regression": safety_regression,
            "integrity_regression": integrity_regression,
            "metrics_digest": metrics_digest,
            "evaluation_input_hashes": evaluation_input_hashes,
        }));
        Self {
            schema_version: "promotion_evidence_v1".into(),
            proposal_id: proposal.proposal_id.clone(),
            held_out_improvement,
            safety_regression,
            integrity_regression,
            metrics,
            evaluation_input_hashes,
            metrics_digest,
            reproducibility_digest,
            rollback_target: rollback_target.into(),
            evaluated_at: Utc::now().to_rfc3339(),
        }
    }

    fn verify_integrity(&self, proposal: &ShadowPolicyProposalV1) -> Result<(), String> {
        if self.schema_version != "promotion_evidence_v1"
            || self.proposal_id != proposal.proposal_id
        {
            return Err("evidence schema or proposal identity mismatch".into());
        }
        if !self.held_out_improvement.is_finite() {
            return Err("held-out improvement is not finite".into());
        }
        if self.metrics_digest != digest_json(&self.metrics) {
            return Err("metrics digest does not match metrics payload".into());
        }
        let expected = digest_json(&serde_json::json!({
            "proposal": proposal.proposal_digest,
            "held_out_improvement": self.held_out_improvement,
            "safety_regression": self.safety_regression,
            "integrity_regression": self.integrity_regression,
            "metrics_digest": self.metrics_digest,
            "evaluation_input_hashes": self.evaluation_input_hashes,
        }));
        if self.reproducibility_digest != expected {
            return Err("reproducibility digest does not match evaluation inputs".into());
        }
        if self.evaluation_input_hashes.is_empty()
            || self
                .evaluation_input_hashes
                .iter()
                .any(|hash| hash.trim().is_empty())
        {
            return Err("held-out evaluation input hashes are required".into());
        }
        if self.rollback_target.trim().is_empty() {
            return Err("explicit rollback target is required".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowPolicyPromotionPermitV1 {
    pub principal: String,
    pub caller_id: String,
    pub capability: String,
    pub elevation: String,
}

impl ShadowPolicyPromotionPermitV1 {
    pub const CAPABILITY: &'static str = "memory.shadow_policy.promote";

    pub fn elevated(
        principal: impl Into<String>,
        caller_id: impl Into<String>,
        elevation: impl Into<String>,
    ) -> Self {
        Self {
            principal: principal.into(),
            caller_id: caller_id.into(),
            capability: Self::CAPABILITY.into(),
            elevation: elevation.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionDispositionV1 {
    Promoted,
    Rejected,
    Quarantined,
    Deferred,
    Expired,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotionDecisionReceiptV1 {
    pub schema_version: String,
    pub receipt_id: String,
    pub caller_idempotency_key: String,
    pub proposal_id: String,
    pub principal: String,
    pub policy_kind: ShadowPolicyKindV1,
    pub disposition: PromotionDispositionV1,
    pub status: ShadowPolicyStatusV1,
    pub reason_codes: Vec<String>,
    pub evidence_digest: String,
    pub before_version: Option<u64>,
    pub after_version: Option<u64>,
    pub before_policy_digest: Option<String>,
    pub after_policy_digest: Option<String>,
    pub rollback_target: Option<String>,
    pub receipt_digest: String,
    pub committed_at: String,
}

impl PromotionDecisionReceiptV1 {
    pub const SCHEMA_VERSION: &'static str = PROMOTION_DECISION_RECEIPT_V1;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveShadowPolicyV1 {
    pub principal: String,
    pub policy_kind: ShadowPolicyKindV1,
    pub version: u64,
    pub policy: Value,
    pub policy_digest: String,
    pub source_proposal_id: String,
    pub activated_by: String,
    pub activated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowExecutionComparisonV1 {
    pub schema_version: String,
    pub principal: String,
    pub policy_kind: ShadowPolicyKindV1,
    pub input_digest: String,
    pub cases_compared: usize,
    pub changed_cases: usize,
    pub baseline_output_digest: String,
    pub shadow_output_digest: String,
    pub served: bool,
    pub canonical_mutation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionGateDecisionV1 {
    pub admissible: bool,
    pub disposition: PromotionDispositionV1,
    pub reason_codes: Vec<String>,
}

/// Evaluate a proposal without persistence or serving effects.
pub fn evaluate_shadow_policy_promotion_v1(
    proposal: &ShadowPolicyProposalV1,
    evidence: &PromotionEvidenceV1,
    permit: &ShadowPolicyPromotionPermitV1,
    active: Option<&ActiveShadowPolicyV1>,
) -> PromotionGateDecisionV1 {
    match evaluate_gate(proposal, evidence, permit, active) {
        Ok(()) => PromotionGateDecisionV1 {
            admissible: true,
            disposition: PromotionDispositionV1::Promoted,
            reason_codes: Vec::new(),
        },
        Err(GateFailure::Expired(reason)) => PromotionGateDecisionV1 {
            admissible: false,
            disposition: PromotionDispositionV1::Expired,
            reason_codes: vec![reason],
        },
        Err(GateFailure::Quarantine(reason)) => PromotionGateDecisionV1 {
            admissible: false,
            disposition: PromotionDispositionV1::Quarantined,
            reason_codes: vec![reason],
        },
        Err(GateFailure::Defer(reason)) => PromotionGateDecisionV1 {
            admissible: false,
            disposition: PromotionDispositionV1::Deferred,
            reason_codes: vec![reason],
        },
        Err(GateFailure::Reject(reason)) => PromotionGateDecisionV1 {
            admissible: false,
            disposition: PromotionDispositionV1::Rejected,
            reason_codes: vec![reason],
        },
    }
}

pub fn compare_shadow_execution_v1(
    principal: impl Into<String>,
    policy_kind: ShadowPolicyKindV1,
    input_digest: impl Into<String>,
    cases: Vec<(&str, Value, Value)>,
) -> Result<ShadowExecutionComparisonV1, MemoryError> {
    let principal = principal.into();
    let input_digest = input_digest.into();
    if principal.trim().is_empty() || input_digest.trim().is_empty() {
        return Err(MemoryError::ShadowPolicyRejected {
            reason: "shadow execution requires principal and input digest".into(),
        });
    }
    let changed_cases = cases
        .iter()
        .filter(|(_, baseline, shadow)| baseline != shadow)
        .count();
    let baseline_outputs: Vec<_> = cases
        .iter()
        .map(|(id, baseline, _)| (*id, baseline))
        .collect();
    let shadow_outputs: Vec<_> = cases.iter().map(|(id, _, shadow)| (*id, shadow)).collect();
    Ok(ShadowExecutionComparisonV1 {
        schema_version: "shadow_execution_comparison_v1".into(),
        principal,
        policy_kind,
        input_digest,
        cases_compared: cases.len(),
        changed_cases,
        baseline_output_digest: digest_json(&serde_json::json!(baseline_outputs)),
        shadow_output_digest: digest_json(&serde_json::json!(shadow_outputs)),
        served: false,
        canonical_mutation: false,
    })
}

impl MemoryStore {
    /// Append a proposal to the shadow ledger. This method cannot write facts, authority state,
    /// active policy, or any runtime configuration.
    pub async fn submit_shadow_policy_proposal(
        &self,
        proposal: ShadowPolicyProposalV1,
    ) -> Result<ShadowPolicyProposalV1, MemoryError> {
        validate_proposal(&proposal)?;
        let proposal_json =
            serde_json::to_string(&proposal).map_err(|e| MemoryError::ShadowPolicyRejected {
                reason: e.to_string(),
            })?;
        let proposal_digest = proposal.proposal_digest.clone();
        self.with_write_conn(move |conn| {
            with_transaction(conn, |tx| {
                let existing: Option<(String, String)> = tx
                    .query_row(
                        "SELECT proposal_json, proposal_digest FROM shadow_policy_proposals
                         WHERE proposal_id = ?1 OR idempotency_key = ?2",
                        params![proposal.proposal_id, proposal.idempotency_key],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                if let Some((json, digest)) = existing {
                    if digest != proposal_digest {
                        return Err(MemoryError::ShadowPolicyConflict {
                            key: proposal.idempotency_key.clone(),
                        });
                    }
                    return serde_json::from_str(&json).map_err(|error| MemoryError::CorruptData {
                        table: "shadow_policy_proposals",
                        row_id: proposal.proposal_id.clone(),
                        detail: error.to_string(),
                    });
                }
                tx.execute(
                    "INSERT INTO shadow_policy_proposals
                     (proposal_id, idempotency_key, principal, policy_kind, proposal_digest,
                      proposal_json, status, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        proposal.proposal_id,
                        proposal.idempotency_key,
                        proposal.principal,
                        proposal.policy_kind.as_str(),
                        proposal_digest,
                        proposal_json,
                        proposal.status.as_str(),
                        proposal.created_at,
                    ],
                )?;
                Ok(proposal)
            })
        })
        .await
    }

    pub async fn get_shadow_policy_proposal(
        &self,
        proposal_id: &str,
        principal: &str,
    ) -> Result<Option<ShadowPolicyProposalV1>, MemoryError> {
        let proposal_id = proposal_id.to_string();
        let principal = principal.to_string();
        self.with_read_conn(move |conn| load_proposal(conn, &proposal_id, &principal))
            .await
    }

    pub async fn get_shadow_policy_promotion_receipt(
        &self,
        caller_idempotency_key: &str,
        principal: &str,
    ) -> Result<Option<PromotionDecisionReceiptV1>, MemoryError> {
        let key = caller_idempotency_key.to_string();
        let principal = principal.to_string();
        self.with_read_conn(move |conn| {
            let raw: Option<String> = conn
                .query_row(
                    "SELECT receipt_json FROM shadow_policy_receipts
                     WHERE caller_idempotency_key = ?1 AND principal = ?2",
                    params![key, principal],
                    |row| row.get(0),
                )
                .optional()?;
            raw.map(|json| {
                serde_json::from_str(&json).map_err(|error| MemoryError::CorruptData {
                    table: "shadow_policy_receipts",
                    row_id: key.clone(),
                    detail: error.to_string(),
                })
            })
            .transpose()
        })
        .await
    }

    pub async fn list_shadow_policy_proposals(
        &self,
        principal: &str,
        policy_kind: Option<ShadowPolicyKindV1>,
    ) -> Result<Vec<ShadowPolicyProposalV1>, MemoryError> {
        let principal = principal.to_string();
        let kind = policy_kind.map(|kind| kind.as_str().to_string());
        self.with_read_conn(move |conn| {
            let mut statement = conn.prepare(
                "SELECT proposal_id, proposal_json FROM shadow_policy_proposals
                 WHERE principal = ?1 AND (?2 IS NULL OR policy_kind = ?2)
                 ORDER BY created_at ASC, proposal_id ASC",
            )?;
            let rows = statement.query_map(params![principal, kind], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut proposals = Vec::new();
            for row in rows {
                let (id, json) = row?;
                let proposal: ShadowPolicyProposalV1 =
                    serde_json::from_str(&json).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?;
                proposals.push(with_current_status(conn, proposal, &id)?);
            }
            Ok(proposals)
        })
        .await
    }

    /// Gate and, if admitted, atomically version active policy and emit its receipt.
    pub async fn promote_shadow_policy(
        &self,
        permit: ShadowPolicyPromotionPermitV1,
        caller_idempotency_key: impl Into<String>,
        proposal_id: impl Into<String>,
        evidence: PromotionEvidenceV1,
    ) -> Result<PromotionDecisionReceiptV1, MemoryError> {
        let caller_idempotency_key = caller_idempotency_key.into();
        let proposal_id = proposal_id.into();
        if permit.capability != ShadowPolicyPromotionPermitV1::CAPABILITY
            || permit.principal.trim().is_empty()
            || permit.caller_id.trim().is_empty()
            || permit.elevation.trim().is_empty()
            || caller_idempotency_key.trim().is_empty()
        {
            return Err(MemoryError::ShadowPolicyUnauthorized {
                principal: permit.principal,
            });
        }
        let fault = self.inner.authority_fault.clone();
        self.with_write_conn(move |conn| {
            // Safety: proposal status observation, gate decision, active-version update, and the
            // immutable receipt commit or roll back as one SQLite transaction.
            with_transaction(conn, |tx| {
                let proposal =
                    load_proposal_tx(tx, &proposal_id, &permit.principal)?.ok_or_else(|| {
                        MemoryError::ShadowPolicyNotFound {
                            proposal_id: proposal_id.clone(),
                        }
                    })?;
                let evidence_digest = evidence_digest(&evidence)?;
                if let Some((stored_digest, receipt_json)) = tx
                    .query_row(
                        "SELECT evidence_digest, receipt_json FROM shadow_policy_receipts
                         WHERE caller_idempotency_key = ?1",
                        params![caller_idempotency_key],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )
                    .optional()?
                {
                    if stored_digest != evidence_digest {
                        return Err(MemoryError::ShadowPolicyConflict {
                            key: caller_idempotency_key.clone(),
                        });
                    }
                    return serde_json::from_str(&receipt_json).map_err(|e| {
                        MemoryError::CorruptData {
                            table: "shadow_policy_receipts",
                            row_id: caller_idempotency_key.clone(),
                            detail: e.to_string(),
                        }
                    });
                }
                let active = load_active_tx(tx, &proposal.principal, proposal.policy_kind)?;
                let gate = evaluate_gate(&proposal, &evidence, &permit, active.as_ref());
                let (disposition, status, reasons) = match gate {
                    Ok(()) => (
                        PromotionDispositionV1::Promoted,
                        ShadowPolicyStatusV1::Promoted,
                        Vec::new(),
                    ),
                    Err(GateFailure::Expired(reason)) => (
                        PromotionDispositionV1::Expired,
                        ShadowPolicyStatusV1::Expired,
                        vec![reason],
                    ),
                    Err(GateFailure::Quarantine(reason)) => (
                        PromotionDispositionV1::Quarantined,
                        ShadowPolicyStatusV1::Quarantined,
                        vec![reason],
                    ),
                    Err(GateFailure::Defer(reason)) => (
                        PromotionDispositionV1::Deferred,
                        ShadowPolicyStatusV1::Deferred,
                        vec![reason],
                    ),
                    Err(GateFailure::Reject(reason)) => (
                        PromotionDispositionV1::Rejected,
                        ShadowPolicyStatusV1::Rejected,
                        vec![reason],
                    ),
                };
                if disposition == PromotionDispositionV1::Promoted {
                    fault_gate(&fault, AuthorityFaultStage::BeforeShadowPromotion)?;
                    let next_policy =
                        apply_delta(&proposal.baseline_policy, &proposal.proposed_delta)?;
                    let next_version = active.as_ref().map_or(1, |current| current.version + 1);
                    let next_digest = digest_json(&next_policy);
                    let now = Utc::now().to_rfc3339();
                    tx.execute(
                        "INSERT INTO shadow_policy_versions
                         (principal, policy_kind, version, policy_json, policy_digest,
                          proposal_id, activated_by, activated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![
                            proposal.principal,
                            proposal.policy_kind.as_str(),
                            next_version,
                            serde_json::to_string(&next_policy).map_err(|e| {
                                MemoryError::ShadowPolicyRejected {
                                    reason: e.to_string(),
                                }
                            })?,
                            next_digest,
                            proposal.proposal_id,
                            permit.caller_id,
                            now,
                        ],
                    )?;
                    tx.execute(
                        "INSERT INTO shadow_active_policies
                         (principal, policy_kind, version, policy_json, policy_digest,
                          source_proposal_id, activated_by, activated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                         ON CONFLICT(principal, policy_kind) DO UPDATE SET
                           version = excluded.version, policy_json = excluded.policy_json,
                           policy_digest = excluded.policy_digest,
                           source_proposal_id = excluded.source_proposal_id,
                           activated_by = excluded.activated_by,
                           activated_at = excluded.activated_at",
                        params![
                            proposal.principal,
                            proposal.policy_kind.as_str(),
                            next_version,
                            serde_json::to_string(&next_policy).map_err(|e| {
                                MemoryError::ShadowPolicyRejected {
                                    reason: e.to_string(),
                                }
                            })?,
                            next_digest,
                            proposal.proposal_id,
                            permit.caller_id,
                            now,
                        ],
                    )?;
                    fault_gate(&fault, AuthorityFaultStage::AfterShadowPromotion)?;
                    let receipt = build_receipt(
                        &proposal,
                        &caller_idempotency_key,
                        &evidence_digest,
                        disposition,
                        status,
                        reasons,
                        active.as_ref(),
                        Some(next_version),
                        Some(next_digest),
                        Some(evidence.rollback_target.clone()),
                    );
                    insert_receipt(tx, &receipt)?;
                    return Ok(receipt);
                }
                let receipt = build_receipt(
                    &proposal,
                    &caller_idempotency_key,
                    &evidence_digest,
                    disposition,
                    status,
                    reasons,
                    active.as_ref(),
                    active.as_ref().map(|a| a.version),
                    active.as_ref().map(|a| a.policy_digest.clone()),
                    Some(evidence.rollback_target),
                );
                insert_receipt(tx, &receipt)?;
                Ok(receipt)
            })
        })
        .await
    }

    pub async fn get_active_shadow_policy(
        &self,
        principal: &str,
        policy_kind: ShadowPolicyKindV1,
    ) -> Result<Option<ActiveShadowPolicyV1>, MemoryError> {
        let principal = principal.to_string();
        self.with_read_conn(move |conn| load_active(conn, &principal, policy_kind))
            .await
    }

    pub async fn rollback_shadow_policy(
        &self,
        permit: ShadowPolicyPromotionPermitV1,
        caller_idempotency_key: impl Into<String>,
        principal: impl Into<String>,
        policy_kind: ShadowPolicyKindV1,
        target_version: u64,
    ) -> Result<PromotionDecisionReceiptV1, MemoryError> {
        let key = caller_idempotency_key.into();
        let principal = principal.into();
        if permit.capability != ShadowPolicyPromotionPermitV1::CAPABILITY
            || permit.principal != principal
            || permit.elevation.trim().is_empty()
            || key.trim().is_empty()
        {
            return Err(MemoryError::ShadowPolicyUnauthorized { principal });
        }
        self.with_write_conn(move |conn| {
            // Safety: target lookup, active pointer restoration, and rollback receipt are one
            // transaction, so a failed restoration cannot expose a partially rolled-back policy.
            with_transaction(conn, |tx| {
                let rollback_identity =
                    digest_json(&serde_json::json!({"target_version": target_version}));
                if let Some((stored_digest, receipt_json)) = tx
                    .query_row(
                        "SELECT evidence_digest, receipt_json FROM shadow_policy_receipts
                         WHERE caller_idempotency_key = ?1 AND principal = ?2",
                        params![key, principal],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )
                    .optional()?
                {
                    if stored_digest != rollback_identity {
                        return Err(MemoryError::ShadowPolicyConflict { key: key.clone() });
                    }
                    return serde_json::from_str(&receipt_json).map_err(|e| {
                        MemoryError::CorruptData {
                            table: "shadow_policy_receipts",
                            row_id: key.clone(),
                            detail: e.to_string(),
                        }
                    });
                }
                let active = load_active_tx(tx, &principal, policy_kind)?.ok_or_else(|| {
                    MemoryError::ShadowPolicyRejected {
                        reason: "no active policy".into(),
                    }
                })?;
                let target = tx
                    .query_row(
                        "SELECT version, policy_json, policy_digest, proposal_id, activated_at
                         FROM shadow_policy_versions
                         WHERE principal = ?1 AND policy_kind = ?2 AND version = ?3",
                        params![principal, policy_kind.as_str(), target_version],
                        |row| {
                            Ok((
                                row.get::<_, u64>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, String>(3)?,
                                row.get::<_, String>(4)?,
                            ))
                        },
                    )
                    .optional()?
                    .ok_or_else(|| MemoryError::ShadowPolicyRejected {
                        reason: "rollback target not found".into(),
                    })?;
                let evidence_digest = rollback_identity;
                if target.0 >= active.version {
                    return Err(MemoryError::ShadowPolicyRejected {
                        reason: "rollback target must be an earlier active version".into(),
                    });
                }
                tx.execute(
                    "INSERT INTO shadow_active_policies
                     (principal, policy_kind, version, policy_json, policy_digest,
                      source_proposal_id, activated_by, activated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(principal, policy_kind) DO UPDATE SET
                       version = excluded.version, policy_json = excluded.policy_json,
                       policy_digest = excluded.policy_digest,
                       source_proposal_id = excluded.source_proposal_id,
                       activated_by = excluded.activated_by,
                       activated_at = excluded.activated_at",
                    params![
                        principal,
                        policy_kind.as_str(),
                        target.0,
                        target.1,
                        target.2,
                        target.3,
                        permit.caller_id,
                        Utc::now().to_rfc3339(),
                    ],
                )?;
                let proposal_id =
                    format!("rollback:{principal}:{:?}:{target_version}", policy_kind);
                let mut receipt = PromotionDecisionReceiptV1 {
                    schema_version: PROMOTION_DECISION_RECEIPT_V1.into(),
                    receipt_id: uuid::Uuid::new_v4().to_string(),
                    caller_idempotency_key: key.clone(),
                    proposal_id,
                    principal: principal.clone(),
                    policy_kind,
                    disposition: PromotionDispositionV1::RolledBack,
                    status: ShadowPolicyStatusV1::RolledBack,
                    reason_codes: vec!["explicit_rollback".into()],
                    evidence_digest,
                    before_version: Some(active.version),
                    after_version: Some(target.0),
                    before_policy_digest: Some(active.policy_digest),
                    after_policy_digest: Some(target.2),
                    rollback_target: Some(target_version.to_string()),
                    receipt_digest: String::new(),
                    committed_at: Utc::now().to_rfc3339(),
                };
                receipt.receipt_digest = digest_json(&receipt_without_digest(&receipt));
                insert_receipt(tx, &receipt)?;
                Ok(receipt)
            })
        })
        .await
    }
}

#[derive(Debug)]
enum GateFailure {
    Expired(String),
    Quarantine(String),
    Defer(String),
    Reject(String),
}

fn validate_proposal(proposal: &ShadowPolicyProposalV1) -> Result<(), MemoryError> {
    if proposal.schema_version != SHADOW_POLICY_PROPOSAL_V1
        || proposal.status != ShadowPolicyStatusV1::Proposed
        || proposal.proposal_id.trim().is_empty()
        || proposal.idempotency_key.trim().is_empty()
        || proposal.principal.trim().is_empty()
        || proposal.provenance.origin.trim().is_empty()
        || proposal.provenance.source.trim().is_empty()
        || proposal.feature_digest.trim().is_empty()
        || proposal
            .training_window
            .held_out_input_digest
            .trim()
            .is_empty()
        || proposal.expires_at.trim().is_empty()
        || proposal.proposal_digest != proposal.compute_digest()
        || proposal.baseline_policy_digest != digest_json(&proposal.baseline_policy)
        || !proposal.risk.score.is_finite()
        || !(0.0..=1.0).contains(&proposal.risk.score)
    {
        return Err(MemoryError::ShadowPolicyRejected {
            reason: "proposal failed structural or digest validation".into(),
        });
    }
    for field in [
        &proposal.training_window.training_start,
        &proposal.training_window.training_end,
        &proposal.training_window.evaluation_start,
        &proposal.training_window.evaluation_end,
    ] {
        if DateTime::parse_from_rfc3339(field).is_err() {
            return Err(MemoryError::ShadowPolicyRejected {
                reason: format!("invalid evaluation window timestamp '{field}'"),
            });
        }
    }
    let training_start = DateTime::parse_from_rfc3339(&proposal.training_window.training_start)
        .map_err(|_| MemoryError::ShadowPolicyRejected {
            reason: "invalid training window".into(),
        })?;
    let training_end = DateTime::parse_from_rfc3339(&proposal.training_window.training_end)
        .map_err(|_| MemoryError::ShadowPolicyRejected {
            reason: "invalid training window".into(),
        })?;
    let evaluation_start = DateTime::parse_from_rfc3339(&proposal.training_window.evaluation_start)
        .map_err(|_| MemoryError::ShadowPolicyRejected {
            reason: "invalid evaluation window".into(),
        })?;
    let evaluation_end = DateTime::parse_from_rfc3339(&proposal.training_window.evaluation_end)
        .map_err(|_| MemoryError::ShadowPolicyRejected {
            reason: "invalid evaluation window".into(),
        })?;
    if training_start >= training_end
        || evaluation_start >= evaluation_end
        || training_end > evaluation_start
    {
        return Err(MemoryError::ShadowPolicyRejected {
            reason: "training and evaluation windows must be ordered and non-overlapping".into(),
        });
    }
    validate_delta_shape(&proposal.proposed_delta)?;
    Ok(())
}

fn evaluate_gate(
    proposal: &ShadowPolicyProposalV1,
    evidence: &PromotionEvidenceV1,
    permit: &ShadowPolicyPromotionPermitV1,
    active: Option<&ActiveShadowPolicyV1>,
) -> Result<(), GateFailure> {
    if permit.principal != proposal.principal {
        return Err(GateFailure::Reject("principal isolation violation".into()));
    }
    if let Ok(expires) = DateTime::parse_from_rfc3339(&proposal.expires_at) {
        if expires.with_timezone(&Utc) <= Utc::now() {
            return Err(GateFailure::Expired("proposal expired".into()));
        }
    } else {
        return Err(GateFailure::Reject("proposal expiry is not RFC3339".into()));
    }
    if let Err(reason) = evidence.verify_integrity(proposal) {
        return Err(GateFailure::Quarantine(reason));
    }
    let evaluated_at = match DateTime::parse_from_rfc3339(&evidence.evaluated_at) {
        Ok(value) => value.with_timezone(&Utc),
        Err(_) => {
            return Err(GateFailure::Quarantine(
                "evidence timestamp is not RFC3339".into(),
            ))
        }
    };
    let now = Utc::now();
    if evaluated_at > now + Duration::minutes(5) {
        return Err(GateFailure::Reject(
            "evidence timestamp is in the future".into(),
        ));
    }
    if now.signed_duration_since(evaluated_at) > Duration::days(30) {
        return Err(GateFailure::Defer("evaluation evidence is stale".into()));
    }
    if proposal.risk.score > SHADOW_PROPOSAL_MAX_RISK {
        return Err(GateFailure::Reject(
            "proposal risk exceeds promotion bound".into(),
        ));
    }
    if evidence.safety_regression || evidence.integrity_regression {
        return Err(GateFailure::Reject("safety or integrity regression".into()));
    }
    if evidence.held_out_improvement <= 0.0 {
        return Err(GateFailure::Defer(
            "held-out improvement is not positive".into(),
        ));
    }
    if evidence
        .evaluation_input_hashes
        .iter()
        .all(|hash| hash != &proposal.training_window.held_out_input_digest)
    {
        return Err(GateFailure::Reject(
            "held-out input digest is not reproducible".into(),
        ));
    }
    match active {
        Some(active) => {
            if proposal.baseline_policy_digest != active.policy_digest {
                return Err(GateFailure::Defer("baseline policy is stale".into()));
            }
            if evidence.rollback_target != active.version.to_string()
                && evidence.rollback_target != active.policy_digest
            {
                return Err(GateFailure::Reject(
                    "rollback target does not identify active version".into(),
                ));
            }
        }
        None if evidence.rollback_target != "none" => {
            return Err(GateFailure::Reject(
                "initial promotion rollback target must be none".into(),
            ));
        }
        None => {}
    }
    if max_numeric_delta(&proposal.proposed_delta) > SHADOW_MAX_DELTA {
        return Err(GateFailure::Reject(
            "proposed delta exceeds deterministic bound".into(),
        ));
    }
    Ok(())
}

fn max_numeric_delta(value: &Value) -> f64 {
    match value {
        Value::Number(number) => number.as_f64().map_or(f64::INFINITY, f64::abs),
        Value::Array(values) => values.iter().map(max_numeric_delta).fold(0.0, f64::max),
        Value::Object(values) => values.values().map(max_numeric_delta).fold(0.0, f64::max),
        _ => 0.0,
    }
}

fn validate_delta_shape(value: &Value) -> Result<(), MemoryError> {
    match value {
        Value::Null | Value::String(_) => Err(MemoryError::ShadowPolicyRejected {
            reason: "policy deltas must not contain null or string values".into(),
        }),
        Value::Number(number) if number.as_f64().is_none() => {
            Err(MemoryError::ShadowPolicyRejected {
                reason: "policy delta contains an invalid number".into(),
            })
        }
        Value::Number(_) | Value::Bool(_) => Ok(()),
        Value::Array(values) => values.iter().try_for_each(validate_delta_shape),
        Value::Object(values) => values.values().try_for_each(validate_delta_shape),
    }
}

fn apply_delta(baseline: &Value, delta: &Value) -> Result<Value, MemoryError> {
    match (baseline, delta) {
        (Value::Object(base), Value::Object(delta)) => {
            let mut result = base.clone();
            for (key, change) in delta {
                let next = match (result.get(key), change) {
                    (Some(Value::Number(old)), Value::Number(change)) => {
                        let old =
                            old.as_f64()
                                .ok_or_else(|| MemoryError::ShadowPolicyRejected {
                                    reason: "baseline number is invalid".into(),
                                })?;
                        let change =
                            change
                                .as_f64()
                                .ok_or_else(|| MemoryError::ShadowPolicyRejected {
                                    reason: "delta number is invalid".into(),
                                })?;
                        serde_json::Number::from_f64(old + change)
                            .map(Value::Number)
                            .ok_or_else(|| MemoryError::ShadowPolicyRejected {
                                reason: "delta produced non-finite policy".into(),
                            })?
                    }
                    (Some(Value::Object(old)), Value::Object(change)) => {
                        apply_delta(&Value::Object(old.clone()), &Value::Object(change.clone()))?
                    }
                    (_, replacement) => replacement.clone(),
                };
                result.insert(key.clone(), next);
            }
            Ok(Value::Object(result))
        }
        (_, replacement) => Ok(replacement.clone()),
    }
}

fn load_proposal(
    conn: &rusqlite::Connection,
    id: &str,
    principal: &str,
) -> Result<Option<ShadowPolicyProposalV1>, MemoryError> {
    let raw: Option<(String, String)> = conn
        .query_row(
            "SELECT proposal_json, proposal_id FROM shadow_policy_proposals
             WHERE proposal_id = ?1 AND principal = ?2",
            params![id, principal],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    raw.map(|(json, id)| {
        let proposal: ShadowPolicyProposalV1 =
            serde_json::from_str(&json).map_err(|e| MemoryError::CorruptData {
                table: "shadow_policy_proposals",
                row_id: id.clone(),
                detail: e.to_string(),
            })?;
        with_current_status(conn, proposal, &id)
    })
    .transpose()
}

fn load_proposal_tx(
    tx: &Transaction<'_>,
    id: &str,
    principal: &str,
) -> Result<Option<ShadowPolicyProposalV1>, MemoryError> {
    load_proposal(tx, id, principal)
}

fn with_current_status(
    conn: &rusqlite::Connection,
    mut proposal: ShadowPolicyProposalV1,
    id: &str,
) -> Result<ShadowPolicyProposalV1, MemoryError> {
    let status: Option<String> = conn.query_row(
        "SELECT status FROM shadow_policy_receipts WHERE proposal_id = ?1 ORDER BY created_at DESC, receipt_id DESC LIMIT 1",
        params![id],
        |row| row.get(0),
    ).optional()?;
    if let Some(status) = status {
        proposal.status = ShadowPolicyStatusV1::parse(&status)?;
    }
    Ok(proposal)
}

fn load_active(
    conn: &rusqlite::Connection,
    principal: &str,
    kind: ShadowPolicyKindV1,
) -> Result<Option<ActiveShadowPolicyV1>, MemoryError> {
    load_active_tx(conn, principal, kind)
}

fn load_active_tx(
    conn: &rusqlite::Connection,
    principal: &str,
    kind: ShadowPolicyKindV1,
) -> Result<Option<ActiveShadowPolicyV1>, MemoryError> {
    let row: Option<(u64, String, String, String, String, String)> = conn.query_row(
        "SELECT version, policy_json, policy_digest, source_proposal_id, activated_by, activated_at
         FROM shadow_active_policies WHERE principal = ?1 AND policy_kind = ?2",
        params![principal, kind.as_str()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
    ).optional()?;
    row.map(
        |(version, policy_json, policy_digest, source_proposal_id, activated_by, activated_at)| {
            let policy =
                serde_json::from_str(&policy_json).map_err(|e| MemoryError::CorruptData {
                    table: "shadow_active_policies",
                    row_id: format!("{principal}:{}", kind.as_str()),
                    detail: e.to_string(),
                })?;
            Ok(ActiveShadowPolicyV1 {
                principal: principal.into(),
                policy_kind: kind,
                version,
                policy,
                policy_digest,
                source_proposal_id,
                activated_by,
                activated_at,
            })
        },
    )
    .transpose()
}

fn build_receipt(
    proposal: &ShadowPolicyProposalV1,
    key: &str,
    evidence_digest: &str,
    disposition: PromotionDispositionV1,
    status: ShadowPolicyStatusV1,
    reason_codes: Vec<String>,
    before: Option<&ActiveShadowPolicyV1>,
    after_version: Option<u64>,
    after_policy_digest: Option<String>,
    rollback_target: Option<String>,
) -> PromotionDecisionReceiptV1 {
    let mut receipt = PromotionDecisionReceiptV1 {
        schema_version: PROMOTION_DECISION_RECEIPT_V1.into(),
        receipt_id: uuid::Uuid::new_v4().to_string(),
        caller_idempotency_key: key.into(),
        proposal_id: proposal.proposal_id.clone(),
        principal: proposal.principal.clone(),
        policy_kind: proposal.policy_kind,
        disposition,
        status,
        reason_codes,
        evidence_digest: evidence_digest.into(),
        before_version: before.map(|policy| policy.version),
        after_version,
        before_policy_digest: before.map(|policy| policy.policy_digest.clone()),
        after_policy_digest,
        rollback_target,
        receipt_digest: String::new(),
        committed_at: Utc::now().to_rfc3339(),
    };
    receipt.receipt_digest = digest_json(&receipt_without_digest(&receipt));
    receipt
}

fn receipt_without_digest(receipt: &PromotionDecisionReceiptV1) -> Value {
    let mut value = serde_json::to_value(receipt).unwrap_or(Value::Null);
    if let Value::Object(ref mut object) = value {
        object.insert("receipt_digest".into(), Value::String(String::new()));
    }
    value
}

fn insert_receipt(
    tx: &Transaction<'_>,
    receipt: &PromotionDecisionReceiptV1,
) -> Result<(), MemoryError> {
    tx.execute(
        "INSERT INTO shadow_policy_receipts
         (receipt_id, caller_idempotency_key, proposal_id, principal, policy_kind,
          evidence_digest, status, receipt_json, receipt_digest, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            receipt.receipt_id,
            receipt.caller_idempotency_key,
            receipt.proposal_id,
            receipt.principal,
            receipt.policy_kind.as_str(),
            receipt.evidence_digest,
            receipt.status.as_str(),
            serde_json::to_string(receipt).map_err(|e| MemoryError::ShadowPolicyRejected {
                reason: e.to_string()
            })?,
            receipt.receipt_digest,
            receipt.committed_at,
        ],
    )?;
    Ok(())
}

fn fault_gate(
    fault: &std::sync::Arc<std::sync::Mutex<Option<AuthorityFaultStage>>>,
    stage: AuthorityFaultStage,
) -> Result<(), MemoryError> {
    let mut guard = fault
        .lock()
        .map_err(|_| MemoryError::Other("authority fault lock poisoned".into()))?;
    if guard.as_ref() == Some(&stage) {
        *guard = None;
        return Err(MemoryError::AuthorityFaultInjected { stage });
    }
    Ok(())
}

fn digest_json(value: &Value) -> String {
    let canonical = canonical_json(value);
    format!("blake3:{}", blake3::hash(canonical.as_bytes()).to_hex())
}

fn evidence_digest(evidence: &PromotionEvidenceV1) -> Result<String, MemoryError> {
    let value = serde_json::to_value(evidence).map_err(|e| MemoryError::ShadowPolicyRejected {
        reason: e.to_string(),
    })?;
    let mut object = value;
    if let Value::Object(ref mut fields) = object {
        // Evaluation wall-clock time is trace metadata, not evidence identity. Excluding it
        // makes a semantically identical retry idempotent while retaining all metric inputs.
        fields.remove("evaluated_at");
    }
    Ok(digest_json(&object))
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into()),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => {
            let mut keys: Vec<_> = values.keys().collect();
            keys.sort();
            let entries = keys
                .into_iter()
                .map(|key| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap_or_else(|_| "\"\"".into()),
                        canonical_json(&values[key])
                    )
                })
                .collect::<Vec<_>>();
            format!("{{{}}}", entries.join(","))
        }
    }
}
