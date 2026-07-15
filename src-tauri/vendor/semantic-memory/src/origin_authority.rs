//! Immutable origin labels and laundering-resistant governed-access decisions.

use crate::{Fact, SearchReplayReportV1, SearchResult, StoredGraphEdge};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const ORIGIN_AUTHORITY_LABEL_V1: &str = "origin_authority_label_v1";
pub const ORIGIN_AUTHORITY_DECISION_V1: &str = "origin_authority_decision_v1";
pub const GOVERNED_ACCESS_POLICY_V1: &str = "governed_access_policy_v1";

/// A principal whose data, consent, or delegated authority is being used.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubjectPrincipalV1(pub String);

impl SubjectPrincipalV1 {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if value.trim().is_empty() {
            Err("subject principal must be non-empty".into())
        } else {
            Ok(Self(value))
        }
    }
}

/// The authenticated principal making an access request now.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CallerPrincipalV1(pub String);

impl CallerPrincipalV1 {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if value.trim().is_empty() {
            Err("caller principal must be non-empty".into())
        } else {
            Ok(Self(value))
        }
    }
}

/// The complete audience asserted at access time. It is a set, never a single ambiguous string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AudienceV1(pub Vec<String>);

impl AudienceV1 {
    pub fn new(mut values: Vec<String>) -> Self {
        values.retain(|value| !value.trim().is_empty());
        values.sort();
        values.dedup();
        Self(values)
    }

    fn intersects(&self, other: &Self) -> bool {
        self.0.iter().any(|value| other.0.contains(value))
    }
}

/// Exact resource scope for v1. Optional fields must agree whenever both sides name them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NamespaceScopeV1 {
    pub namespace: String,
    pub domain: Option<String>,
    pub workspace_id: Option<String>,
    pub repo_id: Option<String>,
}

impl NamespaceScopeV1 {
    pub fn exact(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            ..Self::default()
        }
    }

    pub fn is_bound(&self) -> bool {
        !self.namespace.trim().is_empty()
    }

    fn permits_namespace(&self, namespace: &str) -> bool {
        self.is_bound() && self.namespace == namespace
    }

    fn permits_scope(&self, requested: &Self) -> bool {
        self.is_bound()
            && requested.is_bound()
            && self.namespace == requested.namespace
            && optional_scope_matches(&self.domain, &requested.domain)
            && optional_scope_matches(&self.workspace_id, &requested.workspace_id)
            && optional_scope_matches(&self.repo_id, &requested.repo_id)
    }
}

fn optional_scope_matches(resource: &Option<String>, requested: &Option<String>) -> bool {
    match (resource, requested) {
        (Some(resource), Some(requested)) => resource == requested,
        // A request may not widen an explicitly constrained resource; an unconstrained resource
        // likewise cannot be selected through a more-specific invented scope.
        (None, None) => true,
        _ => false,
    }
}

/// Compatibility shape for a caller-carried lease request.
///
/// This value is never authority: the local crate has no issuer-controlled lease resolver, so
/// governed evaluation fail-closes every delegation/elevation request that carries one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationElevationLeaseV1 {
    pub lease_id: String,
    pub delegator: SubjectPrincipalV1,
    pub delegatee: CallerPrincipalV1,
    pub purposes: Vec<GovernedAccessPurposeV1>,
    pub scope: NamespaceScopeV1,
    pub audience: AudienceV1,
    pub expires_at: String,
    #[serde(default)]
    pub revoked: bool,
    pub elevation: bool,
}

impl DelegationElevationLeaseV1 {
    pub fn delegation(
        lease_id: impl Into<String>,
        delegator: impl Into<String>,
        delegatee: impl Into<String>,
        purposes: Vec<GovernedAccessPurposeV1>,
        scope: NamespaceScopeV1,
        audience: Vec<String>,
        expires_at: impl Into<String>,
    ) -> Self {
        Self {
            lease_id: lease_id.into(),
            delegator: SubjectPrincipalV1(delegator.into()),
            delegatee: CallerPrincipalV1(delegatee.into()),
            purposes,
            scope,
            audience: AudienceV1::new(audience),
            expires_at: expires_at.into(),
            revoked: false,
            elevation: false,
        }
    }

    pub fn elevation(
        lease_id: impl Into<String>,
        subject: impl Into<String>,
        caller: impl Into<String>,
        scope: NamespaceScopeV1,
        expires_at: impl Into<String>,
    ) -> Self {
        Self {
            lease_id: lease_id.into(),
            delegator: SubjectPrincipalV1(subject.into()),
            delegatee: CallerPrincipalV1(caller.into()),
            purposes: vec![GovernedAccessPurposeV1::Admin],
            scope,
            audience: AudienceV1::default(),
            expires_at: expires_at.into(),
            revoked: false,
            elevation: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginRiskV1 {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityScopeV1 {
    Denied,
    PrincipalOnly,
    Audience,
    Universal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityScopesV1 {
    pub recall: AuthorityScopeV1,
    pub assertion: AuthorityScopeV1,
    pub action: AuthorityScopeV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginClassV1 {
    UserStatement,
    ExternalEvidence,
    ToolOutput,
    OperatorSystem,
    Derived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElevationRequirementV1 {
    Never,
    ExplicitOperatorApproval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevocationStatusV1 {
    Active,
    PendingReview,
    Revoked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginDerivationKindV1 {
    Summary,
    Rephrase,
    TrustedToolEcho,
    Corroboration,
    Other,
}

/// Immutable label fixed at the canonical write boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginAuthorityLabelV1 {
    pub schema_version: String,
    pub origin_class: OriginClassV1,
    pub origin_principal: String,
    pub origin_channel: String,
    pub origin_digest: String,
    pub risk: OriginRiskV1,
    pub scopes: AuthorityScopesV1,
    pub elevation: ElevationRequirementV1,
    pub revocation_reference: Option<String>,
    pub revocation_status: RevocationStatusV1,
    pub audience: Vec<String>,
    pub ancestor_digests: Vec<String>,
    pub derivation_kind: Option<OriginDerivationKindV1>,
    /// Principal the assertion is about. Kept separate from the write principal so a caller
    /// cannot substitute a subject merely by presenting an otherwise valid audience.
    #[serde(default)]
    pub subject_principal: Option<SubjectPrincipalV1>,
    /// Audience captured at write time. `audience` above remains for decoding V1 records.
    #[serde(default)]
    pub audience_at_write: AudienceV1,
    /// Immutable resource scope captured at the canonical write boundary.
    #[serde(default)]
    pub resource_scope: NamespaceScopeV1,
    #[serde(default = "default_policy_version")]
    pub policy_version: String,
    #[serde(default)]
    pub policy_digest: String,
}

fn default_policy_version() -> String {
    GOVERNED_ACCESS_POLICY_V1.into()
}

impl OriginAuthorityLabelV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        origin_class: OriginClassV1,
        origin_principal: impl Into<String>,
        origin_channel: impl Into<String>,
        origin_digest: impl Into<String>,
        risk: OriginRiskV1,
        scopes: AuthorityScopesV1,
        elevation: ElevationRequirementV1,
        revocation_reference: Option<String>,
        revocation_status: RevocationStatusV1,
        mut audience: Vec<String>,
    ) -> Result<Self, String> {
        let origin_principal = origin_principal.into();
        let origin_channel = origin_channel.into();
        let origin_digest = origin_digest.into();
        if origin_principal.trim().is_empty()
            || origin_channel.trim().is_empty()
            || origin_digest.trim().is_empty()
        {
            return Err("origin principal, channel, and digest must be non-empty".into());
        }
        if revocation_status != RevocationStatusV1::Active
            && revocation_reference.as_deref().map_or(true, str::is_empty)
        {
            return Err("non-active origin labels require a revocation reference".into());
        }
        audience.retain(|value| !value.trim().is_empty());
        audience.sort();
        audience.dedup();
        let mut label = Self {
            schema_version: ORIGIN_AUTHORITY_LABEL_V1.into(),
            origin_class,
            origin_principal,
            origin_channel,
            origin_digest,
            risk,
            scopes,
            elevation,
            revocation_reference,
            revocation_status,
            audience: audience.clone(),
            ancestor_digests: Vec::new(),
            derivation_kind: None,
            subject_principal: None,
            audience_at_write: AudienceV1::new(audience.clone()),
            resource_scope: NamespaceScopeV1::default(),
            policy_version: default_policy_version(),
            policy_digest: String::new(),
        };
        label.refresh_policy_digest()?;
        Ok(label)
    }

    pub fn with_subject_principal(mut self, subject: SubjectPrincipalV1) -> Self {
        self.subject_principal = Some(subject);
        self.refresh_policy_digest()
            .expect("origin policy label serialization is infallible");
        self
    }

    pub fn with_resource_scope(mut self, scope: NamespaceScopeV1) -> Self {
        self.resource_scope = scope;
        self.refresh_policy_digest()
            .expect("origin policy label serialization is infallible");
        self
    }

    pub(crate) fn bind_resource_scope(mut self, scope: NamespaceScopeV1) -> Result<Self, String> {
        if !self.resource_scope.is_bound() {
            self.resource_scope = scope;
        }
        self.refresh_policy_digest()?;
        Ok(self)
    }

    fn effective_audience(&self) -> AudienceV1 {
        if self.audience_at_write.0.is_empty() {
            AudienceV1::new(self.audience.clone())
        } else {
            self.audience_at_write.clone()
        }
    }

    fn effective_subject(&self) -> SubjectPrincipalV1 {
        self.subject_principal
            .clone()
            .unwrap_or_else(|| SubjectPrincipalV1(self.origin_principal.clone()))
    }

    fn refresh_policy_digest(&mut self) -> Result<(), String> {
        self.policy_digest = self.computed_policy_digest()?;
        Ok(())
    }

    fn computed_policy_digest(&self) -> Result<String, String> {
        let bytes = serde_json::to_vec(&(
            &self.policy_version,
            &self.origin_principal,
            &self.subject_principal,
            self.effective_audience(),
            &self.resource_scope,
            &self.scopes,
            self.elevation,
        ))
        .map_err(|error| format!("serialize access policy label: {error}"))?;
        Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
    }

    /// Deterministically derive a label using maximum ancestor risk and minimum authority.
    pub fn derive(
        ancestors: &[Self],
        kind: OriginDerivationKindV1,
        derived_content_digest: impl Into<String>,
    ) -> Result<Self, String> {
        if ancestors.is_empty() {
            return Err("derived origin requires at least one ancestor".into());
        }
        let derived_content_digest = derived_content_digest.into();
        if derived_content_digest.trim().is_empty() {
            return Err("derived content digest must be non-empty".into());
        }
        let risk = ancestors.iter().map(|label| label.risk).max().unwrap();
        let scopes = AuthorityScopesV1 {
            recall: ancestors
                .iter()
                .map(|label| label.scopes.recall)
                .min()
                .unwrap(),
            assertion: ancestors
                .iter()
                .map(|label| label.scopes.assertion)
                .min()
                .unwrap(),
            action: ancestors
                .iter()
                .map(|label| label.scopes.action)
                .min()
                .unwrap(),
        };
        let principal = if ancestors
            .iter()
            .all(|label| label.origin_principal == ancestors[0].origin_principal)
        {
            ancestors[0].origin_principal.clone()
        } else {
            "multiple-origins".into()
        };
        let mut audience = ancestors[0].audience.clone();
        for ancestor in &ancestors[1..] {
            audience.retain(|entry| ancestor.audience.contains(entry));
        }
        let mut ancestor_digests = ancestors
            .iter()
            .map(label_digest)
            .collect::<Result<Vec<_>, _>>()?;
        ancestor_digests.sort();
        let digest_input = serde_json::to_vec(&(
            "origin-derivation-v1",
            kind,
            &derived_content_digest,
            &ancestor_digests,
            risk,
            &scopes,
        ))
        .map_err(|error| format!("serialize origin derivation: {error}"))?;
        let subject_principal = if ancestors
            .iter()
            .all(|label| label.effective_subject() == ancestors[0].effective_subject())
        {
            Some(ancestors[0].effective_subject())
        } else {
            None
        };
        let resource_scope = if ancestors
            .iter()
            .all(|label| label.resource_scope == ancestors[0].resource_scope)
        {
            ancestors[0].resource_scope.clone()
        } else {
            NamespaceScopeV1::default()
        };
        let mut label = Self {
            schema_version: ORIGIN_AUTHORITY_LABEL_V1.into(),
            origin_class: OriginClassV1::Derived,
            origin_principal: principal,
            origin_channel: format!("derived:{kind:?}").to_lowercase(),
            origin_digest: format!("blake3:{}", blake3::hash(&digest_input).to_hex()),
            risk,
            scopes,
            // Transformations never confer authority; a separate future promotion record would
            // be required even when every ancestor was operator-originated.
            elevation: ElevationRequirementV1::Never,
            revocation_reference: ancestors
                .iter()
                .find_map(|label| label.revocation_reference.clone()),
            revocation_status: ancestors
                .iter()
                .map(|label| label.revocation_status)
                .max_by_key(|status| match status {
                    RevocationStatusV1::Active => 0,
                    RevocationStatusV1::PendingReview => 1,
                    RevocationStatusV1::Revoked => 2,
                })
                .unwrap(),
            audience: audience.clone(),
            ancestor_digests,
            derivation_kind: Some(kind),
            subject_principal,
            audience_at_write: AudienceV1::new(audience.clone()),
            resource_scope,
            policy_version: default_policy_version(),
            policy_digest: String::new(),
        };
        label.refresh_policy_digest()?;
        Ok(label)
    }

    pub fn operator_system(principal: &str, channel: &str) -> Self {
        Self::new(
            OriginClassV1::OperatorSystem,
            principal,
            channel,
            format!(
                "blake3:{}",
                blake3::hash(format!("operator:{principal}:{channel}").as_bytes()).to_hex()
            ),
            OriginRiskV1::Low,
            AuthorityScopesV1 {
                recall: AuthorityScopeV1::Universal,
                assertion: AuthorityScopeV1::Universal,
                action: AuthorityScopeV1::Universal,
            },
            ElevationRequirementV1::ExplicitOperatorApproval,
            None,
            RevocationStatusV1::Active,
            vec![principal.to_string()],
        )
        .expect("operator origin constants are valid")
    }
}

pub fn label_digest(label: &OriginAuthorityLabelV1) -> Result<String, String> {
    let bytes =
        serde_json::to_vec(label).map_err(|error| format!("serialize origin label: {error}"))?;
    Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginAuthorityRecordV1 {
    pub fact_id: String,
    pub label: OriginAuthorityLabelV1,
    pub label_digest: String,
    pub recorded_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GovernedAccessPurposeV1 {
    Recall,
    Assertion,
    Action,
    Export,
    Replay,
    Admin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GovernedAccessRequestV1 {
    /// Compatibility mirror of `caller.0`; governed evaluation rejects disagreement.
    pub principal: String,
    /// Compatibility mirror of the first typed audience member; it is never used as a fallback.
    pub audience: String,
    pub purpose: GovernedAccessPurposeV1,
    /// Compatibility mirror of `scope.namespace`; governed evaluation rejects disagreement.
    pub namespace: String,
    pub caller: CallerPrincipalV1,
    pub subject: SubjectPrincipalV1,
    pub audiences: AudienceV1,
    pub scope: NamespaceScopeV1,
    pub delegation_or_elevation: Option<DelegationElevationLeaseV1>,
    pub policy_version: String,
    pub policy_digest: String,
}

impl GovernedAccessRequestV1 {
    pub fn new(
        principal: impl Into<String>,
        audience: impl Into<String>,
        purpose: GovernedAccessPurposeV1,
        namespace: impl Into<String>,
    ) -> Self {
        let principal = principal.into();
        let audience = audience.into();
        let namespace = namespace.into();
        Self::for_principals(
            CallerPrincipalV1(principal.clone()),
            SubjectPrincipalV1(principal),
            vec![audience],
            purpose,
            NamespaceScopeV1::exact(namespace),
        )
    }

    pub fn for_principals(
        caller: CallerPrincipalV1,
        subject: SubjectPrincipalV1,
        audience: Vec<String>,
        purpose: GovernedAccessPurposeV1,
        scope: NamespaceScopeV1,
    ) -> Self {
        let audiences = AudienceV1::new(audience);
        let policy_version = GOVERNED_ACCESS_POLICY_V1.to_string();
        let policy_digest = access_request_digest(
            &caller,
            &subject,
            &audiences,
            purpose,
            &scope,
            None,
            &policy_version,
        );
        Self {
            principal: caller.0.clone(),
            audience: audiences.0.first().cloned().unwrap_or_default(),
            purpose,
            namespace: scope.namespace.clone(),
            caller,
            subject,
            audiences,
            scope,
            delegation_or_elevation: None,
            policy_version,
            policy_digest,
        }
    }

    pub fn with_delegation_or_elevation(mut self, lease: DelegationElevationLeaseV1) -> Self {
        self.delegation_or_elevation = Some(lease);
        self.policy_digest = access_request_digest(
            &self.caller,
            &self.subject,
            &self.audiences,
            self.purpose,
            &self.scope,
            self.delegation_or_elevation.as_ref(),
            &self.policy_version,
        );
        self
    }

    pub fn with_purpose(mut self, purpose: GovernedAccessPurposeV1) -> Self {
        self.purpose = purpose;
        self.policy_digest = access_request_digest(
            &self.caller,
            &self.subject,
            &self.audiences,
            self.purpose,
            &self.scope,
            self.delegation_or_elevation.as_ref(),
            &self.policy_version,
        );
        self
    }

    pub fn with_audiences(mut self, audience: Vec<String>) -> Self {
        self.audiences = AudienceV1::new(audience);
        self.audience = self.audiences.0.first().cloned().unwrap_or_default();
        self.policy_digest = access_request_digest(
            &self.caller,
            &self.subject,
            &self.audiences,
            self.purpose,
            &self.scope,
            self.delegation_or_elevation.as_ref(),
            &self.policy_version,
        );
        self
    }

    pub fn with_lease_revoked(mut self) -> Self {
        if let Some(lease) = &mut self.delegation_or_elevation {
            lease.revoked = true;
        }
        self.policy_digest = access_request_digest(
            &self.caller,
            &self.subject,
            &self.audiences,
            self.purpose,
            &self.scope,
            self.delegation_or_elevation.as_ref(),
            &self.policy_version,
        );
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginAuthorityDecisionV1 {
    pub schema_version: String,
    pub fact_id: String,
    pub principal: String,
    pub audience_compat: String,
    pub purpose: GovernedAccessPurposeV1,
    pub allowed: bool,
    pub reasons: Vec<String>,
    pub origin_label_digest: Option<String>,
    pub revocation_reference: Option<String>,
    pub decision_digest: String,
    pub caller: CallerPrincipalV1,
    pub subject: SubjectPrincipalV1,
    pub audience: AudienceV1,
    pub scope: NamespaceScopeV1,
    pub policy_version: String,
    pub policy_digest: String,
    pub outcome: PolicyDecisionV1,
    pub lease_id: Option<String>,
}

/// Typed terminal outcome carried by every governed access decision receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecisionV1 {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernedFactAccessV1 {
    pub fact: Option<Fact>,
    pub decision: OriginAuthorityDecisionV1,
    pub origin: Option<OriginAuthorityRecordV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernedSearchResponseV1 {
    pub results: Vec<SearchResult>,
    pub decisions: Vec<OriginAuthorityDecisionV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernedFactListResponseV1 {
    pub facts: Vec<Fact>,
    pub decisions: Vec<OriginAuthorityDecisionV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernedGraphResponseV1 {
    pub edges: Vec<StoredGraphEdge>,
    pub decisions: Vec<OriginAuthorityDecisionV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernedReplayResponseV1 {
    pub replay: SearchReplayReportV1,
    pub allowed_result_ids: Vec<String>,
    pub decisions: Vec<OriginAuthorityDecisionV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernedStateResolutionResponseV1 {
    pub response: crate::ResolvedMemoryAnswerV1,
    pub decisions: Vec<OriginAuthorityDecisionV1>,
}

/// Projection rows have no implicit authority. Until an imported projection carries a durable
/// origin label, governed reads return an empty collection plus denial receipts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernedProjectionResponseV1<T> {
    pub items: Vec<T>,
    pub decisions: Vec<OriginAuthorityDecisionV1>,
}

pub(crate) fn decide(
    fact_id: &str,
    fact_namespace: Option<&str>,
    origin: Option<&OriginAuthorityRecordV1>,
    dynamic_revocation: Option<&str>,
    request: &GovernedAccessRequestV1,
) -> OriginAuthorityDecisionV1 {
    let Some(record) = origin else {
        return evaluate_governed_access_v1(
            fact_id,
            fact_namespace,
            None,
            dynamic_revocation,
            request,
        );
    };
    if label_digest(&record.label).ok().as_deref() != Some(record.label_digest.as_str()) {
        // Make an inconsistent stored record fail in the canonical evaluator rather than
        // treating the database row's stale digest as advisory metadata.
        let mut inconsistent = record.label.clone();
        inconsistent.policy_digest.clear();
        return evaluate_governed_access_v1(
            fact_id,
            fact_namespace,
            Some(&inconsistent),
            dynamic_revocation,
            request,
        );
    }
    evaluate_governed_access_v1(
        fact_id,
        fact_namespace,
        Some(&record.label),
        dynamic_revocation,
        request,
    )
}

/// The sole policy evaluator for every governed read and mutation path. Callers may acquire
/// candidates through caches, replay, graph traversal, or a direct ID, but only this function may
/// authorize returning content. It deliberately has no permissive compatibility branch.
pub fn evaluate_governed_access_v1(
    resource_id: &str,
    resource_namespace: Option<&str>,
    label: Option<&OriginAuthorityLabelV1>,
    dynamic_revocation: Option<&str>,
    request: &GovernedAccessRequestV1,
) -> OriginAuthorityDecisionV1 {
    let mut reasons = Vec::new();
    let mut allowed = true;
    if request.policy_version != GOVERNED_ACCESS_POLICY_V1
        || request.policy_digest
            != access_request_digest(
                &request.caller,
                &request.subject,
                &request.audiences,
                request.purpose,
                &request.scope,
                request.delegation_or_elevation.as_ref(),
                &request.policy_version,
            )
        || request.principal != request.caller.0
        || request.namespace != request.scope.namespace
        || request.audience != request.audiences.0.first().cloned().unwrap_or_default()
        || request.caller.0.trim().is_empty()
        || request.subject.0.trim().is_empty()
        || request.audiences.0.is_empty()
        || !request.scope.is_bound()
    {
        allowed = false;
        reasons.push("invalid_access_request".into());
    }
    let Some(label) = label else {
        allowed = false;
        reasons.push("origin_absent".into());
        return finish_decision(
            resource_id,
            request,
            allowed,
            reasons,
            None,
            dynamic_revocation,
        );
    };
    if label.policy_version != GOVERNED_ACCESS_POLICY_V1
        || label.computed_policy_digest().ok().as_deref() != Some(label.policy_digest.as_str())
    {
        allowed = false;
        reasons.push("policy_label_inconsistent".into());
    }
    if resource_namespace != Some(request.scope.namespace.as_str())
        || !label
            .resource_scope
            .permits_namespace(request.scope.namespace.as_str())
        || !label.resource_scope.permits_scope(&request.scope)
    {
        allowed = false;
        reasons.push("namespace_scope_mismatch".into());
    }
    let revocation_reference = dynamic_revocation
        .map(str::to_string)
        .or_else(|| label.revocation_reference.clone());
    if dynamic_revocation.is_some() || label.revocation_status != RevocationStatusV1::Active {
        allowed = false;
        reasons.push("origin_revoked_or_pending".into());
    }
    let lease = request.delegation_or_elevation.as_ref();
    let valid_delegation = validate_lease(lease, request, false, &mut reasons);
    let valid_elevation = validate_lease(lease, request, true, &mut reasons);
    if request.caller.0 != request.subject.0 && !valid_delegation && !valid_elevation {
        allowed = false;
        reasons.push("caller_subject_delegation_required".into());
    }
    let label_audience = label.effective_audience();
    if !request.audiences.intersects(&label_audience) {
        allowed = false;
        reasons.push("audience_intersection_empty".into());
    }
    if label.origin_class == OriginClassV1::Derived
        && label.subject_principal.is_none()
        && label.origin_principal == "multiple-origins"
    {
        allowed = false;
        reasons.push("cross_principal_derived_subject_ambiguous".into());
    }
    if request.purpose == GovernedAccessPurposeV1::Admin {
        if !valid_elevation {
            allowed = false;
            reasons.push("admin_elevation_required".into());
        }
    } else {
        let scope = match request.purpose {
            GovernedAccessPurposeV1::Recall => label.scopes.recall,
            GovernedAccessPurposeV1::Assertion => label.scopes.assertion,
            GovernedAccessPurposeV1::Action => label.scopes.action,
            GovernedAccessPurposeV1::Export | GovernedAccessPurposeV1::Replay => {
                label.scopes.recall
            }
            GovernedAccessPurposeV1::Admin => unreachable!("admin is handled above"),
        };
        let in_scope = match scope {
            AuthorityScopeV1::Denied => false,
            AuthorityScopeV1::PrincipalOnly => request.caller.0 == label.origin_principal,
            AuthorityScopeV1::Audience => request.audiences.intersects(&label_audience),
            AuthorityScopeV1::Universal => true,
        };
        if !in_scope {
            allowed = false;
            reasons.push("scope_or_principal_denied".into());
        }
    }
    if reasons.is_empty() {
        reasons.push("origin_authority_satisfied".into());
    }
    finish_decision(
        resource_id,
        request,
        allowed,
        reasons,
        label_digest(label).ok(),
        revocation_reference.as_deref(),
    )
}

fn validate_lease(
    lease: Option<&DelegationElevationLeaseV1>,
    request: &GovernedAccessRequestV1,
    require_elevation: bool,
    reasons: &mut Vec<String>,
) -> bool {
    let Some(lease) = lease else {
        return false;
    };
    if lease.elevation != require_elevation {
        return false;
    }
    if lease.revoked {
        reasons.push("delegation_revoked".into());
        return false;
    }
    let expiry = DateTime::parse_from_rfc3339(&lease.expires_at)
        .ok()
        .map(|value| value.with_timezone(&Utc));
    if expiry.map_or(true, |expiry| expiry <= Utc::now()) {
        reasons.push("delegation_expired".into());
        return false;
    }
    if lease.lease_id.trim().is_empty()
        || lease.delegator != request.subject
        || lease.delegatee != request.caller
        || !lease.scope.permits_scope(&request.scope)
        || (!require_elevation && !lease.purposes.contains(&request.purpose))
        || (!lease.audience.0.is_empty() && !request.audiences.intersects(&lease.audience))
    {
        reasons.push("delegation_scope_denied".into());
        return false;
    }
    reasons.push("untrusted_caller_carried_lease".into());
    false
}

fn access_request_digest(
    caller: &CallerPrincipalV1,
    subject: &SubjectPrincipalV1,
    audiences: &AudienceV1,
    purpose: GovernedAccessPurposeV1,
    scope: &NamespaceScopeV1,
    lease: Option<&DelegationElevationLeaseV1>,
    policy_version: &str,
) -> String {
    let bytes = serde_json::to_vec(&(
        policy_version,
        caller,
        subject,
        audiences,
        purpose,
        scope,
        lease,
    ))
    .expect("access policy request serialization cannot fail");
    format!("blake3:{}", blake3::hash(&bytes).to_hex())
}

fn finish_decision(
    fact_id: &str,
    request: &GovernedAccessRequestV1,
    allowed: bool,
    reasons: Vec<String>,
    origin_label_digest: Option<String>,
    revocation_reference: Option<&str>,
) -> OriginAuthorityDecisionV1 {
    let digest_bytes = serde_json::to_vec(&(
        ORIGIN_AUTHORITY_DECISION_V1,
        fact_id,
        request,
        allowed,
        &reasons,
        &origin_label_digest,
        revocation_reference,
    ))
    .expect("decision contract serialization cannot fail");
    OriginAuthorityDecisionV1 {
        schema_version: ORIGIN_AUTHORITY_DECISION_V1.into(),
        fact_id: fact_id.into(),
        principal: request.principal.clone(),
        audience_compat: request.audience.clone(),
        purpose: request.purpose,
        allowed,
        reasons,
        origin_label_digest,
        revocation_reference: revocation_reference.map(str::to_string),
        decision_digest: format!("blake3:{}", blake3::hash(&digest_bytes).to_hex()),
        caller: request.caller.clone(),
        subject: request.subject.clone(),
        audience: request.audiences.clone(),
        scope: request.scope.clone(),
        policy_version: request.policy_version.clone(),
        policy_digest: request.policy_digest.clone(),
        outcome: if allowed {
            PolicyDecisionV1::Allow
        } else {
            PolicyDecisionV1::Deny
        },
        lease_id: request
            .delegation_or_elevation
            .as_ref()
            .map(|lease| lease.lease_id.clone()),
    }
}
