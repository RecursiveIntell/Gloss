//! Governed procedural memory.
//!
//! Procedures are immutable, tested action candidates. They are intentionally stored outside
//! facts, claims, FTS, embeddings, and factual authority lineages. This module exposes no command
//! executor: even a promoted procedure is only returned with a fit/authority decision.

use crate::db::with_transaction;
use crate::origin_authority::{
    evaluate_governed_access_v1, AudienceV1, CallerPrincipalV1, GovernedAccessPurposeV1,
    GovernedAccessRequestV1, NamespaceScopeV1, OriginAuthorityDecisionV1, OriginAuthorityLabelV1,
    SubjectPrincipalV1,
};
use crate::{MemoryError, MemoryStore};
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const PROCEDURAL_MEMORY_ARTIFACT_V1: &str = "procedural_memory_artifact_v1";
pub const PROCEDURE_TEST_RECEIPT_V1: &str = "procedure_test_receipt_v1";
pub const PROCEDURE_LIFECYCLE_RECEIPT_V1: &str = "procedure_lifecycle_receipt_v1";
const PROCEDURE_LIFECYCLE_POLICY_V1: &str = "procedure_lifecycle_policy_v1";
const PROCEDURE_ACTION_POLICY_V1: &str = "procedure_action_policy_v1";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProcedureCapabilityV1 {
    pub domain: String,
    pub name: String,
}

impl ProcedureCapabilityV1 {
    pub fn new(domain: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            domain: domain.into(),
            name: name.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProcedureActionV1 {
    pub kind: String,
    pub description: String,
}

impl ProcedureActionV1 {
    pub fn new(kind: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            description: description.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicabilityOperatorV1 {
    Equals,
    NotEquals,
    Present,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApplicabilityPredicateV1 {
    pub field: String,
    pub operator: ApplicabilityOperatorV1,
    pub value: Option<Value>,
}

impl ApplicabilityPredicateV1 {
    pub fn equals(field: impl Into<String>, value: Value) -> Self {
        Self {
            field: field.into(),
            operator: ApplicabilityOperatorV1::Equals,
            value: Some(value),
        }
    }

    pub fn not_equals(field: impl Into<String>, value: Value) -> Self {
        Self {
            field: field.into(),
            operator: ApplicabilityOperatorV1::NotEquals,
            value: Some(value),
        }
    }

    pub fn present(field: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            operator: ApplicabilityOperatorV1::Present,
            value: None,
        }
    }

    fn matches(&self, context: &Value) -> bool {
        let actual = value_at_path(context, &self.field);
        match self.operator {
            ApplicabilityOperatorV1::Equals => actual == self.value.as_ref(),
            ApplicabilityOperatorV1::NotEquals => actual.is_some() && actual != self.value.as_ref(),
            ApplicabilityOperatorV1::Present => actual.is_some(),
            ApplicabilityOperatorV1::Absent => actual.is_none(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcedurePreconditionV1 {
    pub predicate: ApplicabilityPredicateV1,
    pub failure_code: String,
}

impl ProcedurePreconditionV1 {
    pub fn equals(field: impl Into<String>, value: Value) -> Self {
        let field = field.into();
        Self {
            predicate: ApplicabilityPredicateV1::equals(field.clone(), value),
            failure_code: format!("precondition:{field}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcedureStepV1 {
    pub step_id: String,
    pub sequence: u32,
    pub action_kind: String,
    pub tool: String,
    pub arguments: Value,
    pub rollback: Option<String>,
}

impl ProcedureStepV1 {
    pub fn tool(
        step_id: impl Into<String>,
        tool: impl Into<String>,
        arguments: Value,
        rollback: Option<String>,
    ) -> Self {
        Self {
            step_id: step_id.into(),
            sequence: 1,
            action_kind: "tool_call".into(),
            tool: tool.into(),
            arguments,
            rollback,
        }
    }

    pub fn at_sequence(mut self, sequence: u32) -> Self {
        self.sequence = sequence;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AllowedProcedureToolV1 {
    pub tool: String,
    pub argument_schema: Value,
    pub schema_digest: String,
}

impl AllowedProcedureToolV1 {
    pub fn new(tool: impl Into<String>, argument_schema: Value) -> Self {
        let tool = tool.into();
        let schema_digest = digest_value(&argument_schema);
        Self {
            tool,
            argument_schema,
            schema_digest,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcedureEffectV1 {
    pub kind: String,
    pub value: Value,
}

impl ProcedureEffectV1 {
    pub fn new(kind: impl Into<String>, value: Value) -> Self {
        Self {
            kind: kind.into(),
            value,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcedureRiskV1 {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcedureFixtureV1 {
    pub fixture_id: String,
    pub context: Value,
    pub available_tools: Vec<String>,
    pub expected_effects: Vec<ProcedureEffectV1>,
    pub forbidden_effects: Vec<ProcedureEffectV1>,
}

impl ProcedureFixtureV1 {
    pub fn new(
        fixture_id: impl Into<String>,
        context: Value,
        mut available_tools: Vec<String>,
        expected_effects: Vec<ProcedureEffectV1>,
        forbidden_effects: Vec<ProcedureEffectV1>,
    ) -> Self {
        available_tools.sort();
        available_tools.dedup();
        Self {
            fixture_id: fixture_id.into(),
            context,
            available_tools,
            expected_effects,
            forbidden_effects,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcedureEvidenceTestEnvelopeV1 {
    pub schema_version: String,
    pub sandbox_profile: String,
    pub fixtures: Vec<ProcedureFixtureV1>,
    pub source_fact_ids: Vec<String>,
    pub tested_tool_schema_digests: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcedureRevocationV1 {
    pub revocation_id: String,
    pub reason_digest: String,
    pub revoked_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcedureValidationV1 {
    pub schema_version: String,
    pub artifact_id: String,
    pub artifact_digest: String,
    pub valid: bool,
    pub reason_codes: Vec<String>,
    pub validation_digest: String,
}

impl ProcedureEvidenceTestEnvelopeV1 {
    pub fn new(
        sandbox_profile: impl Into<String>,
        fixtures: Vec<ProcedureFixtureV1>,
        mut source_fact_ids: Vec<String>,
    ) -> Self {
        source_fact_ids.sort();
        source_fact_ids.dedup();
        Self {
            schema_version: "procedure_evidence_test_envelope_v1".into(),
            sandbox_profile: sandbox_profile.into(),
            fixtures,
            source_fact_ids,
            tested_tool_schema_digests: BTreeMap::new(),
        }
    }
}

/// Immutable procedure version. Lifecycle state is recorded separately as append-only events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProceduralMemoryArtifactV1 {
    pub schema_version: String,
    pub artifact_id: String,
    pub capability: ProcedureCapabilityV1,
    pub action: ProcedureActionV1,
    pub applicability: Vec<ApplicabilityPredicateV1>,
    pub preconditions: Vec<ProcedurePreconditionV1>,
    pub steps: Vec<ProcedureStepV1>,
    pub allowed_tools: Vec<AllowedProcedureToolV1>,
    pub expected_effects: Vec<ProcedureEffectV1>,
    pub forbidden_effects: Vec<ProcedureEffectV1>,
    pub risk: ProcedureRiskV1,
    pub origin_authority: OriginAuthorityLabelV1,
    pub principal: String,
    pub audience: AudienceV1,
    pub scope: NamespaceScopeV1,
    pub version: u64,
    pub supersedes: Option<String>,
    pub evidence_test_envelope: ProcedureEvidenceTestEnvelopeV1,
    pub expires_at: Option<String>,
    pub revocation: Option<ProcedureRevocationV1>,
    pub artifact_digest: String,
}

impl ProceduralMemoryArtifactV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        artifact_id: impl Into<String>,
        capability: ProcedureCapabilityV1,
        action: ProcedureActionV1,
        applicability: Vec<ApplicabilityPredicateV1>,
        preconditions: Vec<ProcedurePreconditionV1>,
        mut steps: Vec<ProcedureStepV1>,
        mut allowed_tools: Vec<AllowedProcedureToolV1>,
        expected_effects: Vec<ProcedureEffectV1>,
        forbidden_effects: Vec<ProcedureEffectV1>,
        risk: ProcedureRiskV1,
        origin_authority: OriginAuthorityLabelV1,
        principal: impl Into<String>,
        audience: Vec<String>,
        scope: NamespaceScopeV1,
        version: u64,
        supersedes: Option<String>,
        mut evidence_test_envelope: ProcedureEvidenceTestEnvelopeV1,
        expires_at: Option<String>,
    ) -> Result<Self, String> {
        steps.sort_by_key(|step| step.sequence);
        for (index, step) in steps.iter_mut().enumerate() {
            if step.sequence == 1 && index > 0 {
                step.sequence = u32::try_from(index + 1).map_err(|_| "too many steps")?;
            }
        }
        allowed_tools.sort_by(|a, b| a.tool.cmp(&b.tool));
        evidence_test_envelope.tested_tool_schema_digests = allowed_tools
            .iter()
            .map(|tool| (tool.tool.clone(), tool.schema_digest.clone()))
            .collect();
        let mut artifact = Self {
            schema_version: PROCEDURAL_MEMORY_ARTIFACT_V1.into(),
            artifact_id: artifact_id.into(),
            capability,
            action,
            applicability,
            preconditions,
            steps,
            allowed_tools,
            expected_effects,
            forbidden_effects,
            risk,
            origin_authority,
            principal: principal.into(),
            audience: AudienceV1::new(audience),
            scope,
            version,
            supersedes,
            evidence_test_envelope,
            expires_at,
            revocation: None,
            artifact_digest: String::new(),
        };
        artifact.refresh_digest();
        Ok(artifact)
    }

    pub fn compute_digest(&self) -> String {
        let mut unsigned = self.clone();
        unsigned.artifact_digest.clear();
        digest_serializable(&unsigned)
    }

    pub fn refresh_digest(&mut self) {
        self.artifact_digest = self.compute_digest();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcedureLifecycleDispositionV1 {
    Compiled,
    Tested,
    Promoted,
    Quarantined,
    Revoked,
    RolledBack,
}

impl ProcedureLifecycleDispositionV1 {
    fn as_str(self) -> &'static str {
        match self {
            Self::Compiled => "compiled",
            Self::Tested => "tested",
            Self::Promoted => "promoted",
            Self::Quarantined => "quarantined",
            Self::Revoked => "revoked",
            Self::RolledBack => "rolled_back",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcedureFixtureReceiptV1 {
    pub fixture_id: String,
    pub passed: bool,
    pub reason_codes: Vec<String>,
    pub fixture_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcedureTestReceiptV1 {
    pub schema_version: String,
    pub artifact_id: String,
    pub artifact_digest: String,
    pub sandbox_profile: String,
    pub fixture_count: usize,
    pub fixtures: Vec<ProcedureFixtureReceiptV1>,
    pub passed: bool,
    pub idempotent: bool,
    pub rollback_verified: bool,
    pub test_envelope_digest: String,
    pub receipt_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcedureLifecycleReceiptV1 {
    pub schema_version: String,
    pub receipt_id: String,
    pub caller_idempotency_key: String,
    pub operation: String,
    pub artifact_id: String,
    pub artifact_digest: String,
    pub principal: String,
    pub disposition: ProcedureLifecycleDispositionV1,
    pub reason_codes: Vec<String>,
    pub test_receipt: Option<ProcedureTestReceiptV1>,
    pub prior_event_digest: Option<String>,
    pub event_id: String,
    pub event_digest: String,
    pub receipt_digest: String,
    pub committed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcedureLifecyclePermitV1 {
    pub schema_version: String,
    pub principal: String,
    pub caller_id: String,
    pub capability: String,
    pub elevation: String,
}

impl ProcedureLifecyclePermitV1 {
    pub const CAPABILITY: &'static str = "memory.procedure.lifecycle";

    pub fn elevated(principal: impl Into<String>, caller_id: impl Into<String>) -> Self {
        Self {
            schema_version: PROCEDURE_LIFECYCLE_POLICY_V1.into(),
            principal: principal.into(),
            caller_id: caller_id.into(),
            capability: Self::CAPABILITY.into(),
            elevation: "explicit_operator_approval".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcedureActionPermitV1 {
    pub schema_version: String,
    pub principal: String,
    pub caller_id: String,
    pub capability: String,
    pub scope: NamespaceScopeV1,
    pub elevation: String,
    pub expires_at: String,
}

impl ProcedureActionPermitV1 {
    pub const CAPABILITY: &'static str = "memory.procedure.action";

    pub fn elevated(
        principal: impl Into<String>,
        caller_id: impl Into<String>,
        scope: NamespaceScopeV1,
    ) -> Self {
        Self {
            schema_version: PROCEDURE_ACTION_POLICY_V1.into(),
            principal: principal.into(),
            caller_id: caller_id.into(),
            capability: Self::CAPABILITY.into(),
            scope,
            elevation: "explicit_operator_approval".into(),
            expires_at: "2999-01-01T00:00:00Z".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcedureAccessPathV1 {
    Search,
    DirectId,
    Cache,
    Export,
    Replay,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcedureRetrievalRequestV1 {
    pub schema_version: String,
    pub capability: ProcedureCapabilityV1,
    pub action: ProcedureActionV1,
    pub context: Value,
    pub caller: CallerPrincipalV1,
    pub subject: SubjectPrincipalV1,
    pub audience: AudienceV1,
    pub scope: NamespaceScopeV1,
    pub purpose: GovernedAccessPurposeV1,
    pub access_path: ProcedureAccessPathV1,
    pub artifact_id: Option<String>,
    pub action_permit: Option<ProcedureActionPermitV1>,
}

impl ProcedureRetrievalRequestV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        capability: ProcedureCapabilityV1,
        action: ProcedureActionV1,
        context: Value,
        caller: CallerPrincipalV1,
        subject: SubjectPrincipalV1,
        audience: Vec<String>,
        scope: NamespaceScopeV1,
        purpose: GovernedAccessPurposeV1,
        access_path: ProcedureAccessPathV1,
    ) -> Self {
        Self {
            schema_version: "procedure_retrieval_request_v1".into(),
            capability,
            action,
            context,
            caller,
            subject,
            audience: AudienceV1::new(audience),
            scope,
            purpose,
            access_path,
            artifact_id: None,
            action_permit: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GovernedProcedureDecisionV1 {
    pub schema_version: String,
    pub artifact_id: Option<String>,
    pub candidate_fit: bool,
    pub authority_allowed: bool,
    pub action_allowed: bool,
    pub test_envelope_passed: bool,
    pub access_path: ProcedureAccessPathV1,
    pub reason_codes: Vec<String>,
    pub origin_decision: Option<OriginAuthorityDecisionV1>,
    pub decision_digest: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GovernedProcedureRetrievalV1 {
    pub schema_version: String,
    pub candidate: Option<ProceduralMemoryArtifactV1>,
    pub decision: GovernedProcedureDecisionV1,
    pub receipt_digest: String,
}

impl MemoryStore {
    /// Compile and persist an immutable procedure version. Invalid input is durably quarantined.
    pub async fn compile_procedure(
        &self,
        artifact: ProceduralMemoryArtifactV1,
        caller_idempotency_key: impl Into<String>,
    ) -> Result<ProcedureLifecycleReceiptV1, MemoryError> {
        let key = caller_idempotency_key.into();
        let payload_digest = digest_serializable(&("compile", &artifact));
        let static_validation = validate_artifact(&artifact);
        self.with_write_conn(move |conn| {
            // Safety: immutable artifact insertion, derivation links, event, and receipt are one
            // atomic write; a failure cannot expose a partially compiled procedure.
            with_transaction(conn, |tx| {
                if let Some(receipt) = load_idempotent_receipt(tx, &key, &payload_digest)? {
                    return Ok(receipt);
                }
                let artifact_json = serde_json::to_string(&artifact).map_err(rejected)?;
                let existing: Option<String> = tx.query_row(
                    "SELECT artifact_digest FROM procedural_memory_artifacts WHERE artifact_id = ?1",
                    params![artifact.artifact_id], |row| row.get(0),
                ).optional()?;
                if existing.as_deref().is_some_and(|digest| digest != artifact.artifact_digest) {
                    return Err(MemoryError::ProceduralMemoryConflict { key });
                }
                if existing.is_none() {
                    tx.execute(
                        "INSERT INTO procedural_memory_artifacts
                         (artifact_id, principal, capability_domain, capability_name, action_kind,
                          version, supersedes, artifact_digest, artifact_json, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        params![artifact.artifact_id, artifact.principal, artifact.capability.domain,
                            artifact.capability.name, artifact.action.kind, artifact.version,
                            artifact.supersedes, artifact.artifact_digest, artifact_json, now()],
                    ).map_err(|error| MemoryError::ProceduralMemoryRejected { reason: error.to_string() })?;
                    for fact_id in &artifact.evidence_test_envelope.source_fact_ids {
                        let fact_id = fact_id.strip_prefix("fact:").unwrap_or(fact_id);
                        tx.execute(
                            "INSERT INTO derivation_edges
                             (source_kind, source_id, target_kind, target_id, derivation_type,
                              invalidation_mode, recorded_at)
                             VALUES ('fact', ?1, 'procedure', ?2, 'procedure_evidence',
                                     'on_source_change', ?3)",
                            params![fact_id, artifact.artifact_id, now()],
                        )?;
                    }
                }
                let source_validation = validate_source_evidence(tx, &artifact);
                let (disposition, reasons) = match (static_validation, source_validation) {
                    (Ok(()), Ok(())) => (ProcedureLifecycleDispositionV1::Compiled, vec![]),
                    (static_result, source_result) => {
                        let mut reasons = static_result.err().unwrap_or_default();
                        reasons.extend(source_result.err().unwrap_or_default());
                        reasons.sort();
                        reasons.dedup();
                        (ProcedureLifecycleDispositionV1::Quarantined, reasons)
                    }
                };
                append_lifecycle(tx, &key, "compile", &payload_digest, &artifact,
                    disposition, reasons, None)
            })
        }).await
    }

    /// Run deterministic fixture simulation. No external tool or command is invoked.
    pub async fn test_procedure(
        &self,
        artifact_id: &str,
        caller_idempotency_key: impl Into<String>,
    ) -> Result<ProcedureLifecycleReceiptV1, MemoryError> {
        let artifact_id = artifact_id.to_string();
        let key = caller_idempotency_key.into();
        let payload_digest = digest_serializable(&("test", &artifact_id));
        self.with_write_conn(move |conn| {
            // Safety: the deterministic test event and its witnessed receipt commit together.
            with_transaction(conn, |tx| {
                if let Some(receipt) = load_idempotent_receipt(tx, &key, &payload_digest)? {
                    return Ok(receipt);
                }
                let artifact = load_artifact_tx(tx, &artifact_id)?.ok_or_else(|| {
                    MemoryError::ProceduralMemoryNotFound {
                        artifact_id: artifact_id.clone(),
                    }
                })?;
                require_latest(
                    tx,
                    &artifact_id,
                    &[ProcedureLifecycleDispositionV1::Compiled],
                )?;
                let test = run_fixture_tests(&artifact);
                let (disposition, reasons) = if test.passed {
                    (ProcedureLifecycleDispositionV1::Tested, vec![])
                } else {
                    (
                        ProcedureLifecycleDispositionV1::Quarantined,
                        test.fixtures
                            .iter()
                            .flat_map(|f| f.reason_codes.clone())
                            .collect(),
                    )
                };
                append_lifecycle(
                    tx,
                    &key,
                    "test",
                    &payload_digest,
                    &artifact,
                    disposition,
                    reasons,
                    Some(test),
                )
            })
        })
        .await
    }

    pub async fn promote_procedure(
        &self,
        permit: ProcedureLifecyclePermitV1,
        artifact_id: &str,
        caller_idempotency_key: impl Into<String>,
    ) -> Result<ProcedureLifecycleReceiptV1, MemoryError> {
        lifecycle_control(
            self,
            permit,
            artifact_id.to_string(),
            caller_idempotency_key.into(),
            "promote",
            ProcedureLifecycleDispositionV1::Promoted,
            None,
        )
        .await
    }

    pub async fn quarantine_procedure(
        &self,
        permit: ProcedureLifecyclePermitV1,
        artifact_id: &str,
        caller_idempotency_key: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<ProcedureLifecycleReceiptV1, MemoryError> {
        lifecycle_control(
            self,
            permit,
            artifact_id.to_string(),
            caller_idempotency_key.into(),
            "quarantine",
            ProcedureLifecycleDispositionV1::Quarantined,
            Some(reason.into()),
        )
        .await
    }

    pub async fn revoke_procedure(
        &self,
        permit: ProcedureLifecyclePermitV1,
        artifact_id: &str,
        caller_idempotency_key: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<ProcedureLifecycleReceiptV1, MemoryError> {
        lifecycle_control(
            self,
            permit,
            artifact_id.to_string(),
            caller_idempotency_key.into(),
            "revoke",
            ProcedureLifecycleDispositionV1::Revoked,
            Some(reason.into()),
        )
        .await
    }

    /// Roll back an active promotion without mutating or deleting the immutable version.
    pub async fn rollback_procedure(
        &self,
        permit: ProcedureLifecyclePermitV1,
        artifact_id: &str,
        caller_idempotency_key: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<ProcedureLifecycleReceiptV1, MemoryError> {
        lifecycle_control(
            self,
            permit,
            artifact_id.to_string(),
            caller_idempotency_key.into(),
            "rollback",
            ProcedureLifecycleDispositionV1::RolledBack,
            Some(reason.into()),
        )
        .await
    }

    /// Return at most one procedure candidate plus a witnessed fit/authority decision.
    /// This function never invokes the procedure or any tool named by it.
    pub async fn retrieve_procedure(
        &self,
        request: ProcedureRetrievalRequestV1,
    ) -> Result<GovernedProcedureRetrievalV1, MemoryError> {
        self.with_read_conn(move |conn| retrieve_governed(conn, request))
            .await
    }
}

async fn lifecycle_control(
    store: &MemoryStore,
    permit: ProcedureLifecyclePermitV1,
    artifact_id: String,
    key: String,
    operation: &'static str,
    disposition: ProcedureLifecycleDispositionV1,
    reason: Option<String>,
) -> Result<ProcedureLifecycleReceiptV1, MemoryError> {
    validate_lifecycle_permit(&permit)?;
    let payload_digest = digest_serializable(&(operation, &permit, &artifact_id, &reason));
    store
        .with_write_conn(move |conn| {
            // Safety: lifecycle transition and immutable receipt are committed atomically.
            with_transaction(conn, |tx| {
                if let Some(receipt) = load_idempotent_receipt(tx, &key, &payload_digest)? {
                    return Ok(receipt);
                }
                let artifact = load_artifact_tx(tx, &artifact_id)?.ok_or_else(|| {
                    MemoryError::ProceduralMemoryNotFound {
                        artifact_id: artifact_id.clone(),
                    }
                })?;
                if artifact.principal != permit.principal {
                    return Err(MemoryError::ProceduralMemoryUnauthorized {
                        principal: permit.principal,
                    });
                }
                let latest = latest_disposition(tx, &artifact_id)?
                    .ok_or_else(|| rejected("missing compile event"))?;
                match disposition {
                    ProcedureLifecycleDispositionV1::Promoted => {
                        if latest != ProcedureLifecycleDispositionV1::Tested {
                            return Err(rejected(
                                "promotion requires the latest deterministic test to pass",
                            ));
                        }
                        if let Some(previous_id) = artifact.supersedes.as_ref() {
                            let previous = load_artifact_tx(tx, previous_id)?
                                .ok_or_else(|| rejected("superseded version not found"))?;
                            if previous.principal != artifact.principal
                                || previous.capability != artifact.capability
                                || previous.action.kind != artifact.action.kind
                                || previous.version.checked_add(1) != Some(artifact.version)
                                || latest_disposition(tx, previous_id)?
                                    != Some(ProcedureLifecycleDispositionV1::Promoted)
                            {
                                return Err(rejected(
                                    "supersession must name the active prior immutable version",
                                ));
                            }
                        } else if artifact.version != 1 {
                            return Err(rejected("version greater than one requires supersedes"));
                        }
                        if artifact.expires_at.as_deref().is_some_and(|value| {
                            DateTime::parse_from_rfc3339(value)
                                .ok()
                                .map_or(true, |expiry| expiry.with_timezone(&Utc) <= Utc::now())
                        }) {
                            return Err(rejected("expired procedure cannot be promoted"));
                        }
                        let forgotten = tx.query_row(
                            "SELECT EXISTS(SELECT 1 FROM forgetting_artifact_invalidations
                             WHERE surface_kind = 'procedure' AND artifact_id = ?1)",
                            params![artifact.artifact_id],
                            |row| row.get::<_, bool>(0),
                        )?;
                        if forgotten || validate_source_evidence(tx, &artifact).is_err() {
                            return Err(rejected("procedure evidence is stale or forgotten"));
                        }
                    }
                    ProcedureLifecycleDispositionV1::Quarantined => {
                        if latest == ProcedureLifecycleDispositionV1::Revoked {
                            return Err(rejected("revoked procedure cannot transition"));
                        }
                    }
                    ProcedureLifecycleDispositionV1::Revoked => {
                        if latest == ProcedureLifecycleDispositionV1::Revoked {
                            return Err(rejected("procedure is already revoked"));
                        }
                    }
                    ProcedureLifecycleDispositionV1::RolledBack => {
                        if latest != ProcedureLifecycleDispositionV1::Promoted {
                            return Err(rejected("rollback requires the active promoted version"));
                        }
                    }
                    _ => return Err(rejected("invalid controlled lifecycle transition")),
                }
                append_lifecycle(
                    tx,
                    &key,
                    operation,
                    &payload_digest,
                    &artifact,
                    disposition,
                    reason
                        .into_iter()
                        .map(|text| format!("reason:{}", digest_serializable(&text)))
                        .collect(),
                    None,
                )
            })
        })
        .await
}

fn retrieve_governed(
    conn: &rusqlite::Connection,
    request: ProcedureRetrievalRequestV1,
) -> Result<GovernedProcedureRetrievalV1, MemoryError> {
    let artifact = load_candidate(conn, &request)?;
    let mut reasons = Vec::new();
    let path_purpose_valid = match request.access_path {
        ProcedureAccessPathV1::Export => request.purpose == GovernedAccessPurposeV1::Export,
        ProcedureAccessPathV1::Replay => request.purpose == GovernedAccessPurposeV1::Replay,
        ProcedureAccessPathV1::Search
        | ProcedureAccessPathV1::DirectId
        | ProcedureAccessPathV1::Cache => matches!(
            request.purpose,
            GovernedAccessPurposeV1::Recall | GovernedAccessPurposeV1::Action
        ),
    };
    if request.schema_version != "procedure_retrieval_request_v1"
        || !request.context.is_object()
        || !path_purpose_valid
    {
        reasons.push("invalid_request_or_access_path_purpose".into());
    }
    let mut fit = false;
    let mut authority_allowed = false;
    let mut action_allowed = false;
    let mut tested = false;
    let mut origin_decision = None;
    let candidate = if let Some(artifact) = artifact {
        let current = artifact.expires_at.as_deref().map_or(true, |value| {
            DateTime::parse_from_rfc3339(value)
                .ok()
                .is_some_and(|expiry| expiry.with_timezone(&Utc) > Utc::now())
        });
        if !current {
            reasons.push("procedure_expired".into());
        }
        fit = reasons.is_empty()
            && artifact.artifact_digest == artifact.compute_digest()
            && artifact.capability == request.capability
            && artifact.action.kind == request.action.kind
            && artifact
                .applicability
                .iter()
                .all(|item| item.matches(&request.context))
            && artifact
                .preconditions
                .iter()
                .all(|item| item.predicate.matches(&request.context));
        if !fit {
            reasons.push("applicability_or_precondition_failed".into());
        }
        tested = latest_passing_test(conn, &artifact.artifact_id)?.is_some();
        if !tested {
            reasons.push("passing_test_envelope_absent".into());
        }
        let access = GovernedAccessRequestV1::for_principals(
            request.caller.clone(),
            request.subject.clone(),
            request.audience.0.clone(),
            request.purpose,
            request.scope.clone(),
        );
        let decision = evaluate_governed_access_v1(
            &artifact.artifact_id,
            Some(&artifact.scope.namespace),
            Some(&artifact.origin_authority),
            artifact
                .revocation
                .as_ref()
                .map(|item| item.revocation_id.as_str()),
            &access,
        );
        authority_allowed = decision.allowed && artifact.principal == request.caller.0;
        if !authority_allowed {
            reasons.push("origin_or_principal_denied".into());
        }
        origin_decision = Some(decision);
        action_allowed = request.purpose != GovernedAccessPurposeV1::Action
            || action_permit_allows(request.action_permit.as_ref(), &request, &artifact);
        if request.purpose == GovernedAccessPurposeV1::Action && !action_allowed {
            reasons.push("explicit_action_elevation_required".into());
        }
        if current && fit && authority_allowed && tested {
            Some(artifact)
        } else {
            None
        }
    } else {
        reasons.push("no_current_promoted_candidate".into());
        None
    };
    if reasons.is_empty() {
        reasons.push("procedure_candidate_governed".into());
    }
    let decision_digest = digest_serializable(&(
        "governed_procedure_decision_v1",
        candidate.as_ref().map(|a| &a.artifact_id),
        fit,
        authority_allowed,
        action_allowed,
        tested,
        request.access_path,
        &reasons,
        &origin_decision,
    ));
    let decision = GovernedProcedureDecisionV1 {
        schema_version: "governed_procedure_decision_v1".into(),
        artifact_id: candidate.as_ref().map(|a| a.artifact_id.clone()),
        candidate_fit: fit,
        authority_allowed,
        action_allowed: action_allowed && fit && authority_allowed && tested,
        test_envelope_passed: tested,
        access_path: request.access_path,
        reason_codes: reasons,
        origin_decision,
        decision_digest,
    };
    let receipt_digest =
        digest_serializable(&("governed_procedure_retrieval_v1", &candidate, &decision));
    Ok(GovernedProcedureRetrievalV1 {
        schema_version: "governed_procedure_retrieval_v1".into(),
        candidate,
        decision,
        receipt_digest,
    })
}

fn load_candidate(
    conn: &rusqlite::Connection,
    request: &ProcedureRetrievalRequestV1,
) -> Result<Option<ProceduralMemoryArtifactV1>, MemoryError> {
    let sql = String::from(
        "SELECT a.artifact_json FROM procedural_memory_artifacts a
         WHERE a.capability_domain = ?1 AND a.capability_name = ?2 AND a.action_kind = ?3
           AND (?4 IS NULL OR a.artifact_id = ?4)
           AND (SELECT e.disposition FROM procedural_memory_events e
                WHERE e.artifact_id = a.artifact_id ORDER BY e.rowid DESC LIMIT 1) = 'promoted'
           AND NOT EXISTS (SELECT 1 FROM procedural_memory_artifacts newer
                           WHERE newer.supersedes = a.artifact_id
                             AND EXISTS (SELECT 1 FROM procedural_memory_events promoted
                                         WHERE promoted.artifact_id = newer.artifact_id
                                           AND promoted.disposition = 'promoted')
                             AND (SELECT pe.disposition FROM procedural_memory_events pe
                                  WHERE pe.artifact_id = newer.artifact_id
                                  ORDER BY pe.rowid DESC LIMIT 1) != 'rolled_back')
           AND NOT EXISTS (SELECT 1 FROM forgetting_artifact_invalidations fi
                           WHERE fi.surface_kind = 'procedure' AND fi.artifact_id = a.artifact_id)
         ORDER BY a.version DESC, a.artifact_id ASC LIMIT 1",
    );
    // The SQL is constant and every selector remains parameterized; access path never chooses a
    // weaker query. Keeping one statement is the direct-ID/cache/export/replay bypass defense.
    let raw: Option<String> = conn
        .query_row(
            &sql,
            params![
                request.capability.domain,
                request.capability.name,
                request.action.kind,
                request.artifact_id
            ],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|json| {
        serde_json::from_str(&json).map_err(|error| MemoryError::CorruptData {
            table: "procedural_memory_artifacts",
            row_id: request.artifact_id.clone().unwrap_or_default(),
            detail: error.to_string(),
        })
    })
    .transpose()
}

fn validate_artifact(artifact: &ProceduralMemoryArtifactV1) -> Result<(), Vec<String>> {
    let mut reasons = Vec::new();
    if artifact.schema_version != PROCEDURAL_MEMORY_ARTIFACT_V1
        || artifact.artifact_id.trim().is_empty()
        || artifact.capability.domain.trim().is_empty()
        || artifact.capability.name.trim().is_empty()
        || artifact.action.kind.trim().is_empty()
        || artifact.action.description.trim().is_empty()
        || artifact.principal.trim().is_empty()
        || !artifact.scope.is_bound()
        || artifact.audience.0.is_empty()
        || artifact.version == 0
    {
        reasons.push("required_typed_field_missing".into());
    }
    if artifact.artifact_digest != artifact.compute_digest() {
        reasons.push("artifact_digest_mismatch".into());
    }
    if artifact.origin_authority.origin_principal != artifact.principal
        || artifact.origin_authority.scopes.assertion != crate::AuthorityScopeV1::Denied
        || artifact.origin_authority.resource_scope != artifact.scope
        || artifact.origin_authority.revocation_status != crate::RevocationStatusV1::Active
    {
        reasons.push("origin_authority_or_nonassertion_boundary_invalid".into());
    }
    if artifact.version == 1 && artifact.supersedes.is_some()
        || artifact.version > 1 && artifact.supersedes.as_deref().map_or(true, str::is_empty)
    {
        reasons.push("version_supersession_invalid".into());
    }
    if artifact
        .expires_at
        .as_deref()
        .is_some_and(|value| DateTime::parse_from_rfc3339(value).is_err())
    {
        reasons.push("expiry_invalid".into());
    }
    if artifact.steps.is_empty()
        || artifact.allowed_tools.is_empty()
        || artifact.expected_effects.is_empty()
        || artifact.evidence_test_envelope.fixtures.is_empty()
    {
        reasons.push("steps_tools_effects_and_tests_required".into());
    }
    let mut sequences = BTreeSet::new();
    let mut step_ids = BTreeSet::new();
    let allowed: BTreeMap<_, _> = artifact
        .allowed_tools
        .iter()
        .map(|tool| (&tool.tool, tool))
        .collect();
    let used_tools: BTreeSet<_> = artifact
        .steps
        .iter()
        .map(|step| step.tool.as_str())
        .collect();
    let allowed_names: BTreeSet<_> = artifact
        .allowed_tools
        .iter()
        .map(|tool| tool.tool.as_str())
        .collect();
    if used_tools != allowed_names {
        reasons.push("tool_manifest_widening".into());
    }
    if artifact.allowed_tools.iter().any(|tool| {
        prohibited_tool(&tool.tool) || schema_has_command_surface(&tool.argument_schema)
    }) {
        reasons.push("general_purpose_execution_surface_forbidden".into());
    }
    for (index, step) in artifact.steps.iter().enumerate() {
        if step.sequence != u32::try_from(index + 1).unwrap_or(u32::MAX)
            || !sequences.insert(step.sequence)
            || !step_ids.insert(&step.step_id)
        {
            reasons.push("steps_not_strictly_ordered".into());
        }
        if prohibited_tool(&step.tool) || step.action_kind != "tool_call" {
            reasons.push("malicious_or_unsupported_step".into());
        }
        match allowed.get(&step.tool) {
            Some(tool) => {
                if tool.schema_digest != digest_value(&tool.argument_schema)
                    || validate_arguments(&step.arguments, &tool.argument_schema).is_err()
                {
                    reasons.push("argument_schema_drift_or_widening".into());
                }
            }
            None => reasons.push("tool_not_in_manifest".into()),
        }
        if artifact.risk >= ProcedureRiskV1::Medium
            && step.rollback.as_deref().map_or(true, str::is_empty)
        {
            reasons.push("rollback_required_for_elevated_risk".into());
        }
    }
    let manifest_digests: BTreeMap<_, _> = artifact
        .allowed_tools
        .iter()
        .map(|tool| (tool.tool.clone(), tool.schema_digest.clone()))
        .collect();
    if artifact.evidence_test_envelope.tested_tool_schema_digests != manifest_digests {
        reasons.push("tested_tool_schema_drift".into());
    }
    for effect in &artifact.forbidden_effects {
        if artifact.expected_effects.contains(effect) {
            reasons.push("expected_forbidden_effect_collision".into());
        }
    }
    for fixture in &artifact.evidence_test_envelope.fixtures {
        let fixture_tools: BTreeSet<_> =
            fixture.available_tools.iter().map(String::as_str).collect();
        if fixture_tools != allowed_names {
            reasons.push("fixture_tool_manifest_widening".into());
        }
    }
    if artifact.evidence_test_envelope.schema_version != "procedure_evidence_test_envelope_v1"
        || artifact.evidence_test_envelope.sandbox_profile != "sandbox-v1"
    {
        reasons.push("unapproved_sandbox_profile".into());
    }
    reasons.sort();
    reasons.dedup();
    if reasons.is_empty() {
        Ok(())
    } else {
        Err(reasons)
    }
}

fn validate_source_evidence(
    tx: &Transaction<'_>,
    artifact: &ProceduralMemoryArtifactV1,
) -> Result<(), Vec<String>> {
    let mut reasons = Vec::new();
    for raw_id in &artifact.evidence_test_envelope.source_fact_ids {
        let fact_id = raw_id.strip_prefix("fact:").unwrap_or(raw_id);
        let row: Result<Option<(String, Option<String>, bool, bool)>, rusqlite::Error> = tx
            .query_row(
                "SELECT f.namespace, o.label_json,
                        EXISTS(SELECT 1 FROM forgotten_facts ff WHERE ff.fact_id = f.id),
                        EXISTS(SELECT 1 FROM origin_authority_revocations r WHERE r.fact_id = f.id)
                 FROM facts f LEFT JOIN origin_authority_labels o ON o.fact_id = f.id
                 WHERE f.id = ?1",
                params![fact_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional();
        match row {
            Ok(Some((namespace, Some(label_json), false, false))) => {
                let label: Result<OriginAuthorityLabelV1, _> = serde_json::from_str(&label_json);
                if label.as_ref().map_or(true, |label| {
                    label.origin_principal != artifact.principal
                        || namespace != artifact.scope.namespace
                        || label.revocation_status != crate::RevocationStatusV1::Active
                }) {
                    reasons.push("source_fact_authority_or_scope_mismatch".into());
                }
            }
            Ok(_) => reasons.push("source_fact_missing_revoked_or_forgotten".into()),
            Err(_) => reasons.push("source_fact_validation_failed".into()),
        }
    }
    reasons.sort();
    reasons.dedup();
    if reasons.is_empty() {
        Ok(())
    } else {
        Err(reasons)
    }
}

/// Deterministically validate a procedure without storing or executing it.
pub fn validate_procedure_artifact_v1(
    artifact: &ProceduralMemoryArtifactV1,
) -> ProcedureValidationV1 {
    let mut reasons = validate_artifact(artifact).err().unwrap_or_default();
    reasons.sort();
    reasons.dedup();
    let valid = reasons.is_empty();
    let validation_digest = digest_serializable(&(
        "procedure_validation_v1",
        &artifact.artifact_id,
        &artifact.artifact_digest,
        valid,
        &reasons,
    ));
    ProcedureValidationV1 {
        schema_version: "procedure_validation_v1".into(),
        artifact_id: artifact.artifact_id.clone(),
        artifact_digest: artifact.artifact_digest.clone(),
        valid,
        reason_codes: reasons,
        validation_digest,
    }
}

fn run_fixture_tests(artifact: &ProceduralMemoryArtifactV1) -> ProcedureTestReceiptV1 {
    let first = evaluate_fixture_set(artifact);
    let second = evaluate_fixture_set(artifact);
    let idempotent = first == second;
    let passed = idempotent && first.iter().all(|fixture| fixture.passed);
    let rollback_verified = artifact
        .steps
        .iter()
        .all(|step| step.rollback.as_deref().is_some_and(|v| !v.is_empty()));
    let envelope_digest = digest_serializable(&artifact.evidence_test_envelope);
    let mut receipt = ProcedureTestReceiptV1 {
        schema_version: PROCEDURE_TEST_RECEIPT_V1.into(),
        artifact_id: artifact.artifact_id.clone(),
        artifact_digest: artifact.artifact_digest.clone(),
        sandbox_profile: artifact.evidence_test_envelope.sandbox_profile.clone(),
        fixture_count: first.len(),
        fixtures: first,
        passed,
        idempotent,
        rollback_verified,
        test_envelope_digest: envelope_digest,
        receipt_digest: String::new(),
    };
    receipt.receipt_digest = digest_serializable(&(
        &receipt.schema_version,
        &receipt.artifact_id,
        &receipt.artifact_digest,
        &receipt.sandbox_profile,
        receipt.fixture_count,
        &receipt.fixtures,
        receipt.passed,
        receipt.idempotent,
        receipt.rollback_verified,
        &receipt.test_envelope_digest,
    ));
    receipt
}

fn evaluate_fixture_set(artifact: &ProceduralMemoryArtifactV1) -> Vec<ProcedureFixtureReceiptV1> {
    artifact
        .evidence_test_envelope
        .fixtures
        .iter()
        .map(|fixture| {
            let mut reasons = Vec::new();
            if !artifact
                .applicability
                .iter()
                .all(|p| p.matches(&fixture.context))
                || !artifact
                    .preconditions
                    .iter()
                    .all(|p| p.predicate.matches(&fixture.context))
            {
                reasons.push("fixture_not_applicable".into());
            }
            if artifact
                .steps
                .iter()
                .any(|step| !fixture.available_tools.contains(&step.tool))
            {
                reasons.push("fixture_tool_unavailable".into());
            }
            if fixture.expected_effects != artifact.expected_effects {
                reasons.push("expected_effect_mismatch".into());
            }
            if fixture
                .forbidden_effects
                .iter()
                .any(|effect| artifact.expected_effects.contains(effect))
                || artifact
                    .forbidden_effects
                    .iter()
                    .any(|effect| fixture.expected_effects.contains(effect))
            {
                reasons.push("forbidden_effect_observed".into());
            }
            reasons.sort();
            reasons.dedup();
            let fixture_digest = digest_serializable(&(
                &fixture.fixture_id,
                &fixture.context,
                &fixture.available_tools,
                &fixture.expected_effects,
                &fixture.forbidden_effects,
                &reasons,
            ));
            ProcedureFixtureReceiptV1 {
                fixture_id: fixture.fixture_id.clone(),
                passed: reasons.is_empty(),
                reason_codes: reasons,
                fixture_digest,
            }
        })
        .collect()
}

fn validate_arguments(arguments: &Value, schema: &Value) -> Result<(), ()> {
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err(());
    }
    let args = arguments.as_object().ok_or(())?;
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or(())?;
    if schema.get("additionalProperties").and_then(Value::as_bool) != Some(false) {
        return Err(());
    }
    let required: BTreeSet<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .ok_or(())?
        .iter()
        .map(|value| value.as_str().ok_or(()))
        .collect::<Result<_, _>>()?;
    if required.iter().any(|field| !args.contains_key(*field))
        || args.keys().any(|field| !properties.contains_key(field))
    {
        return Err(());
    }
    for (field, value) in args {
        let expected = properties
            .get(field)
            .and_then(|v| v.get("type"))
            .and_then(Value::as_str)
            .ok_or(())?;
        let matches = match expected {
            "boolean" => value.is_boolean(),
            "string" => value.is_string(),
            "number" => value.is_number(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "array" => value.is_array(),
            "object" => value.is_object(),
            _ => false,
        };
        if !matches {
            return Err(());
        }
    }
    Ok(())
}

fn prohibited_tool(tool: &str) -> bool {
    let normalized = tool.trim().to_ascii_lowercase();
    [
        "shell",
        "bash",
        "powershell",
        "command",
        "exec",
        "process",
        "terminal",
    ]
    .iter()
    .any(|part| normalized.contains(part))
        || matches!(normalized.as_str(), "sh" | "cmd")
}

fn schema_has_command_surface(schema: &Value) -> bool {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(|properties| {
            properties.keys().any(|key| {
                matches!(
                    key.to_ascii_lowercase().as_str(),
                    "command" | "cmd" | "shell" | "script" | "program" | "executable"
                )
            })
        })
}

fn action_permit_allows(
    permit: Option<&ProcedureActionPermitV1>,
    request: &ProcedureRetrievalRequestV1,
    artifact: &ProceduralMemoryArtifactV1,
) -> bool {
    let Some(permit) = permit else {
        return false;
    };
    permit.schema_version == PROCEDURE_ACTION_POLICY_V1
        && permit.capability == ProcedureActionPermitV1::CAPABILITY
        && permit.principal == request.caller.0
        && permit.principal == artifact.principal
        && !permit.caller_id.trim().is_empty()
        && permit.elevation == "explicit_operator_approval"
        && permit.scope == request.scope
        && permit.scope == artifact.scope
        && DateTime::parse_from_rfc3339(&permit.expires_at)
            .ok()
            .is_some_and(|expiry| expiry.with_timezone(&Utc) > Utc::now())
}

fn validate_lifecycle_permit(permit: &ProcedureLifecyclePermitV1) -> Result<(), MemoryError> {
    if permit.schema_version != PROCEDURE_LIFECYCLE_POLICY_V1
        || permit.capability != ProcedureLifecyclePermitV1::CAPABILITY
        || permit.principal.trim().is_empty()
        || permit.caller_id.trim().is_empty()
        || permit.elevation != "explicit_operator_approval"
    {
        Err(MemoryError::ProceduralMemoryUnauthorized {
            principal: permit.principal.clone(),
        })
    } else {
        Ok(())
    }
}

fn append_lifecycle(
    tx: &Transaction<'_>,
    key: &str,
    operation: &str,
    payload_digest: &str,
    artifact: &ProceduralMemoryArtifactV1,
    disposition: ProcedureLifecycleDispositionV1,
    mut reasons: Vec<String>,
    test_receipt: Option<ProcedureTestReceiptV1>,
) -> Result<ProcedureLifecycleReceiptV1, MemoryError> {
    if key.trim().is_empty() {
        return Err(rejected("idempotency key is required"));
    }
    reasons.sort();
    reasons.dedup();
    let prior_event_digest: Option<String> = tx.query_row(
        "SELECT event_digest FROM procedural_memory_events WHERE artifact_id = ?1 ORDER BY rowid DESC LIMIT 1",
        params![artifact.artifact_id], |row| row.get(0)).optional()?;
    let committed_at = now();
    let event_id = uuid::Uuid::new_v4().to_string();
    let reason_digest = digest_serializable(&reasons);
    let event_digest = digest_serializable(&(
        "procedure_lifecycle_event_v1",
        &event_id,
        &artifact.artifact_id,
        disposition,
        &reason_digest,
        &test_receipt,
        &prior_event_digest,
        &committed_at,
    ));
    tx.execute(
        "INSERT INTO procedural_memory_events
         (event_id, artifact_id, disposition, reason_digest, test_receipt_json,
          prior_event_digest, event_digest, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            event_id,
            artifact.artifact_id,
            disposition.as_str(),
            reason_digest,
            test_receipt
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(rejected)?,
            prior_event_digest,
            event_digest,
            committed_at
        ],
    )?;
    let mut receipt = ProcedureLifecycleReceiptV1 {
        schema_version: PROCEDURE_LIFECYCLE_RECEIPT_V1.into(),
        receipt_id: uuid::Uuid::new_v4().to_string(),
        caller_idempotency_key: key.into(),
        operation: operation.into(),
        artifact_id: artifact.artifact_id.clone(),
        artifact_digest: artifact.artifact_digest.clone(),
        principal: artifact.principal.clone(),
        disposition,
        reason_codes: reasons,
        test_receipt,
        prior_event_digest,
        event_id,
        event_digest,
        receipt_digest: String::new(),
        committed_at,
    };
    receipt.receipt_digest = digest_serializable(&(
        &receipt.schema_version,
        &receipt.receipt_id,
        &receipt.caller_idempotency_key,
        &receipt.operation,
        &receipt.artifact_id,
        &receipt.artifact_digest,
        &receipt.principal,
        receipt.disposition,
        &receipt.reason_codes,
        &receipt.test_receipt,
        &receipt.prior_event_digest,
        &receipt.event_id,
        &receipt.event_digest,
        &receipt.committed_at,
    ));
    tx.execute(
        "INSERT INTO procedural_memory_receipts
         (receipt_id, caller_idempotency_key, operation, payload_digest, artifact_id,
          receipt_json, receipt_digest, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            receipt.receipt_id,
            key,
            operation,
            payload_digest,
            artifact.artifact_id,
            serde_json::to_string(&receipt).map_err(rejected)?,
            receipt.receipt_digest,
            receipt.committed_at
        ],
    )?;
    Ok(receipt)
}

fn load_idempotent_receipt(
    tx: &Transaction<'_>,
    key: &str,
    payload_digest: &str,
) -> Result<Option<ProcedureLifecycleReceiptV1>, MemoryError> {
    let stored: Option<(String, String)> = tx.query_row(
        "SELECT payload_digest, receipt_json FROM procedural_memory_receipts WHERE caller_idempotency_key = ?1",
        params![key], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
    let Some((stored_digest, json)) = stored else {
        return Ok(None);
    };
    if stored_digest != payload_digest {
        return Err(MemoryError::ProceduralMemoryConflict { key: key.into() });
    }
    serde_json::from_str(&json)
        .map(Some)
        .map_err(|error| MemoryError::CorruptData {
            table: "procedural_memory_receipts",
            row_id: key.into(),
            detail: error.to_string(),
        })
}

fn load_artifact_tx(
    tx: &Transaction<'_>,
    artifact_id: &str,
) -> Result<Option<ProceduralMemoryArtifactV1>, MemoryError> {
    let raw: Option<String> = tx
        .query_row(
            "SELECT artifact_json FROM procedural_memory_artifacts WHERE artifact_id = ?1",
            params![artifact_id],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|json| {
        serde_json::from_str(&json).map_err(|error| MemoryError::CorruptData {
            table: "procedural_memory_artifacts",
            row_id: artifact_id.into(),
            detail: error.to_string(),
        })
    })
    .transpose()
}

fn latest_disposition(
    tx: &Transaction<'_>,
    artifact_id: &str,
) -> Result<Option<ProcedureLifecycleDispositionV1>, MemoryError> {
    let raw: Option<String> = tx.query_row(
        "SELECT disposition FROM procedural_memory_events WHERE artifact_id = ?1 ORDER BY rowid DESC LIMIT 1",
        params![artifact_id], |row| row.get(0)).optional()?;
    raw.map(|value| parse_disposition(&value)).transpose()
}

fn require_latest(
    tx: &Transaction<'_>,
    artifact_id: &str,
    allowed: &[ProcedureLifecycleDispositionV1],
) -> Result<(), MemoryError> {
    let latest = latest_disposition(tx, artifact_id)?;
    if latest.is_some_and(|state| allowed.contains(&state)) {
        Ok(())
    } else {
        Err(rejected("invalid lifecycle predecessor"))
    }
}

fn latest_passing_test(
    conn: &rusqlite::Connection,
    artifact_id: &str,
) -> Result<Option<ProcedureTestReceiptV1>, MemoryError> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT test_receipt_json FROM procedural_memory_events
         WHERE artifact_id = ?1 AND disposition = 'tested' AND test_receipt_json IS NOT NULL
         ORDER BY rowid DESC LIMIT 1",
            params![artifact_id],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|json| {
        serde_json::from_str(&json).map_err(|error| MemoryError::CorruptData {
            table: "procedural_memory_events",
            row_id: artifact_id.into(),
            detail: error.to_string(),
        })
    })
    .transpose()
}

fn parse_disposition(value: &str) -> Result<ProcedureLifecycleDispositionV1, MemoryError> {
    match value {
        "compiled" => Ok(ProcedureLifecycleDispositionV1::Compiled),
        "tested" => Ok(ProcedureLifecycleDispositionV1::Tested),
        "promoted" => Ok(ProcedureLifecycleDispositionV1::Promoted),
        "quarantined" => Ok(ProcedureLifecycleDispositionV1::Quarantined),
        "revoked" => Ok(ProcedureLifecycleDispositionV1::Revoked),
        "rolled_back" => Ok(ProcedureLifecycleDispositionV1::RolledBack),
        _ => Err(rejected("unknown lifecycle disposition")),
    }
}

/// Verify the content-addressed deterministic fixture-test receipt.
pub fn verify_procedure_test_receipt_v1(receipt: &ProcedureTestReceiptV1) -> bool {
    receipt.schema_version == PROCEDURE_TEST_RECEIPT_V1
        && receipt.receipt_digest
            == digest_serializable(&(
                &receipt.schema_version,
                &receipt.artifact_id,
                &receipt.artifact_digest,
                &receipt.sandbox_profile,
                receipt.fixture_count,
                &receipt.fixtures,
                receipt.passed,
                receipt.idempotent,
                receipt.rollback_verified,
                &receipt.test_envelope_digest,
            ))
}

/// Verify a witnessed lifecycle receipt and its embedded fixture receipt, when present.
pub fn verify_procedure_lifecycle_receipt_v1(receipt: &ProcedureLifecycleReceiptV1) -> bool {
    let reason_digest = digest_serializable(&receipt.reason_codes);
    let expected_event_digest = digest_serializable(&(
        "procedure_lifecycle_event_v1",
        &receipt.event_id,
        &receipt.artifact_id,
        receipt.disposition,
        &reason_digest,
        &receipt.test_receipt,
        &receipt.prior_event_digest,
        &receipt.committed_at,
    ));
    receipt.schema_version == PROCEDURE_LIFECYCLE_RECEIPT_V1
        && receipt
            .test_receipt
            .as_ref()
            .map_or(true, verify_procedure_test_receipt_v1)
        && receipt.event_digest == expected_event_digest
        && receipt.receipt_digest
            == digest_serializable(&(
                &receipt.schema_version,
                &receipt.receipt_id,
                &receipt.caller_idempotency_key,
                &receipt.operation,
                &receipt.artifact_id,
                &receipt.artifact_digest,
                &receipt.principal,
                receipt.disposition,
                &receipt.reason_codes,
                &receipt.test_receipt,
                &receipt.prior_event_digest,
                &receipt.event_id,
                &receipt.event_digest,
                &receipt.committed_at,
            ))
}

fn value_at_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(value, |current, part| current.as_object()?.get(part))
}

fn digest_value(value: &Value) -> String {
    digest_serializable(value)
}

fn digest_serializable<T: Serialize + ?Sized>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).expect("versioned procedure contracts serialize");
    format!("blake3:{}", blake3::hash(&bytes).to_hex())
}

fn rejected(error: impl ToString) -> MemoryError {
    MemoryError::ProceduralMemoryRejected {
        reason: error.to_string(),
    }
}

fn now() -> String {
    Utc::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string()
}
