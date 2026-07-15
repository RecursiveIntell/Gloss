use async_trait::async_trait;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use stack_ids::{
    ApplicabilityContextId, ApprovalGrantId, ApprovalRecordId, ArtifactId, AttemptId,
    AttestationEnvelopeId, CompiledObligationSetId, CompositionReceiptId, ContentDigest,
    CrossRuntimeReplayTicketId, DigestBuilder, EffectCommitDecisionId, EffectExecutionReceiptId,
    EffectiveConstitutionId, ExecutionPermitId, PolicyDecisionId, ProfileSetId,
    RemoteOracleLeaseId, RemoteSliceResultId, ScopeKey, ToolEffectDispatchReceiptId, TraceCtx,
    TrialId,
};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;

#[async_trait]
pub trait CompensatingAction: Send + Sync {
    async fn compensate(&self, receipt: &ToolReceipt) -> Result<(), ToolError>;
}

#[derive(Debug)]
pub struct ControlData<T>(pub(crate) T);

#[derive(Debug, Clone)]
pub struct UntrustedData<T>(pub T);

/// Accepts only trusted control data.
///
/// `UntrustedData` cannot be passed into this function directly.
///
/// ```compile_fail
/// use llm_tool_runtime::{accept_control_data, UntrustedData};
///
/// let untrusted = UntrustedData("payload".to_string());
/// // Type mismatch: this function expects `ControlData`, not `UntrustedData`.
/// accept_control_data(untrusted);
/// ```
pub fn accept_control_data<T>(data: ControlData<T>) -> T {
    data.0
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionPermitScope {
    namespace: String,
    target_key: String,
}

impl ToolExecutionPermitScope {
    /// Returns the namespace bound by this execution permit.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Returns the concrete target key bound by this execution permit.
    pub fn target_key(&self) -> &str {
        &self.target_key
    }
}

#[derive(Debug)]
pub struct ToolExecutionPermit {
    execution_permit_id: ExecutionPermitId,
    decision_id: PolicyDecisionId,
    approval_record_id: Option<ApprovalRecordId>,
    scope: ToolExecutionPermitScope,
    expires_at: Option<DateTime<Utc>>,
    nonce: String,
    consumed: AtomicBool,
    method_digest: ContentDigest,
    effect_digest: ContentDigest,
}

impl ToolExecutionPermit {
    /// Builds a runtime execution permit snapshot for effectful tool dispatch.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        execution_permit_id: ExecutionPermitId,
        decision_id: PolicyDecisionId,
        approval_record_id: Option<ApprovalRecordId>,
        namespace: impl Into<String>,
        target_key: impl Into<String>,
        method_digest: ContentDigest,
        effect_digest: ContentDigest,
        expires_at: Option<DateTime<Utc>>,
        nonce: impl Into<String>,
    ) -> Self {
        Self {
            execution_permit_id,
            decision_id,
            approval_record_id,
            scope: ToolExecutionPermitScope {
                namespace: namespace.into(),
                target_key: target_key.into(),
            },
            expires_at,
            nonce: nonce.into(),
            consumed: AtomicBool::new(false),
            method_digest,
            effect_digest,
        }
    }

    /// Returns the execution permit identifier.
    pub fn execution_permit_id(&self) -> &ExecutionPermitId {
        &self.execution_permit_id
    }

    /// Returns the policy decision lineage bound to this permit.
    pub fn decision_id(&self) -> &PolicyDecisionId {
        &self.decision_id
    }

    /// Returns the optional approval-record lineage bound to this permit.
    pub fn approval_record_id(&self) -> Option<&ApprovalRecordId> {
        self.approval_record_id.as_ref()
    }

    /// Returns the namespace/target scope enforced by this permit.
    pub fn scope(&self) -> &ToolExecutionPermitScope {
        &self.scope
    }

    /// Returns the expiry instant, when the issuing policy imposed one.
    pub fn expires_at(&self) -> Option<&DateTime<Utc>> {
        self.expires_at.as_ref()
    }

    /// Returns the issuer-provided replay nonce.
    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    /// Returns the tool method digest bound to this permit.
    pub fn method_digest(&self) -> &ContentDigest {
        &self.method_digest
    }

    /// Returns the typed effect digest bound to this permit.
    pub fn effect_digest(&self) -> &ContentDigest {
        &self.effect_digest
    }

    /// Validates expiry and exact method/effect bindings without consuming the permit.
    pub fn validate_binding(
        &self,
        method_digest: &ContentDigest,
        effect_digest: &ContentDigest,
        now: DateTime<Utc>,
    ) -> Result<(), ToolError> {
        if self
            .expires_at
            .as_ref()
            .is_some_and(|expiry| expiry <= &now)
        {
            return Err(ToolError::new(
                ToolErrorClass::Denied,
                "execution permit expired",
            ));
        }
        if &self.method_digest != method_digest || &self.effect_digest != effect_digest {
            return Err(ToolError::new(
                ToolErrorClass::Denied,
                "execution permit method/effect binding mismatch",
            ));
        }
        Ok(())
    }

    /// Atomically consumes this one-shot permit.
    pub fn consume(&self) -> Result<(), ToolError> {
        if self
            .expires_at
            .as_ref()
            .is_some_and(|expiry| expiry <= &Utc::now())
        {
            return Err(ToolError::new(
                ToolErrorClass::Denied,
                "execution permit expired",
            ));
        }
        if self
            .consumed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ToolError::new(
                ToolErrorClass::Denied,
                "execution permit already consumed",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolBackendKind {
    LocalFunction,
    OpenAiFunction,
    OpenAiBuiltIn,
    OllamaFunction,
    RemoteMcp,
}

impl ToolBackendKind {
    /// Returns the stable snake_case label used in serialized receipts.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LocalFunction => "local_function",
            Self::OpenAiFunction => "open_ai_function",
            Self::OpenAiBuiltIn => "open_ai_built_in",
            Self::OllamaFunction => "ollama_function",
            Self::RemoteMcp => "remote_mcp",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutputMode {
    StructuredJson,
    Text,
    ArtifactRefs,
    JobHandle,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ToolSideEffectClass {
    ReadOnly,
    Analysis,
    PreviewWrite,
    Write,
    Admin,
}

impl fmt::Display for ToolSideEffectClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ReadOnly => "read-only",
            Self::Analysis => "analysis",
            Self::PreviewWrite => "preview-write",
            Self::Write => "write",
            Self::Admin => "admin",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolIdempotencyClass {
    Idempotent,
    BestEffort,
    NonIdempotent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolApprovalKind {
    None,
    UserRequired,
    PolicyRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExposureMode {
    Auto,
    OptIn,
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpSurfaceKind {
    None,
    Tool,
    Resource,
    Prompt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolReceiptPersistence {
    #[default]
    Durable,
    Ephemeral,
    /// Legacy name for durable Forge raw-receipt persistence.
    ForgeRaw,
}

impl ToolReceiptPersistence {
    /// Returns whether this mode requires durable preflight and outcome receipts.
    pub fn is_durable(&self) -> bool {
        matches!(self, Self::Durable | Self::ForgeRaw)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOriginKind {
    OpenAiResponses,
    OpenAiChat,
    OllamaChat,
    Local,
    Mcp,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPlannerStage {
    InitialRouting,
    Retrieval,
    Analysis,
    Execution,
    Audit,
}

impl ToolPlannerStage {
    /// Returns the stable snake_case label used in serialized planning metadata.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InitialRouting => "initial_routing",
            Self::Retrieval => "retrieval",
            Self::Analysis => "analysis",
            Self::Execution => "execution",
            Self::Audit => "audit",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolApprovalState {
    NotRequired,
    Approved,
    Denied,
}

impl ToolApprovalState {
    /// Returns the stable snake_case label used in serialized approval metadata.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::Approved => "approved",
            Self::Denied => "denied",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRetryOwner {
    LlmPipeline,
    AgentGraph,
    JobQueue,
    AiBatchQueue,
    ForgeOrchestration,
    External,
}

impl ToolRetryOwner {
    /// Returns the stable snake_case label used in serialized retry-owner metadata.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LlmPipeline => "llm_pipeline",
            Self::AgentGraph => "agent_graph",
            Self::JobQueue => "job_queue",
            Self::AiBatchQueue => "ai_batch_queue",
            Self::ForgeOrchestration => "forge_orchestration",
            Self::External => "external",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalGrantEffectClass {
    Analysis,
    PreviewWrite,
    Write,
    Admin,
}

impl ApprovalGrantEffectClass {
    /// Maps a tool-side effect class into the approval-grant vocabulary.
    pub fn for_tool_side_effect_class(side_effect_class: &ToolSideEffectClass) -> Option<Self> {
        match side_effect_class {
            ToolSideEffectClass::ReadOnly => None,
            ToolSideEffectClass::Analysis => Some(Self::Analysis),
            ToolSideEffectClass::PreviewWrite => Some(Self::PreviewWrite),
            ToolSideEffectClass::Write => Some(Self::Write),
            ToolSideEffectClass::Admin => Some(Self::Admin),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalGrantCitation {
    pub applicability_context_id: ApplicabilityContextId,
    pub profile_set_id: ProfileSetId,
    pub composition_receipt_id: CompositionReceiptId,
    pub effective_constitution_id: EffectiveConstitutionId,
    pub compiled_obligation_set_id: CompiledObligationSetId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalGrantScope {
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_key: Option<String>,
    pub tool_name: String,
    pub effect_class: ApprovalGrantEffectClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner_stage: Option<ToolPlannerStage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalGrant {
    pub grant_id: ApprovalGrantId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_record_id: Option<ApprovalRecordId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<PolicyDecisionId>,
    pub policy_version: String,
    pub scope: ApprovalGrantScope,
    #[serde(default)]
    pub approver_lineage: Vec<String>,
    pub approved_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub citation: ApprovalGrantCitation,
}

impl ApprovalGrant {
    /// Builds a typed approval grant for one tool surface.
    pub fn new(
        policy_version: impl Into<String>,
        scope: ApprovalGrantScope,
        approver_lineage: Vec<String>,
        approved_at: impl Into<String>,
        expires_at: Option<String>,
        citation: ApprovalGrantCitation,
    ) -> Self {
        Self {
            grant_id: ApprovalGrantId::generate(),
            approval_record_id: None,
            decision_id: None,
            policy_version: policy_version.into(),
            scope,
            approver_lineage,
            approved_at: approved_at.into(),
            expires_at,
            citation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolErrorClass {
    InvalidArguments,
    UnknownTool,
    ApprovalRequired,
    Denied,
    Timeout,
    Cancelled,
    OutputTooLarge,
    Execution,
    ReceiptPersistence,
    ProviderContract,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolExposurePolicy {
    pub exposure_mode: ToolExposureMode,
    #[serde(default)]
    pub allow_parallel_calls: bool,
    #[serde(default)]
    pub max_calls_per_turn: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_planner_stages: Vec<ToolPlannerStage>,
}

impl Default for ToolExposurePolicy {
    fn default() -> Self {
        Self {
            exposure_mode: ToolExposureMode::Auto,
            allow_parallel_calls: false,
            max_calls_per_turn: 1,
            allowed_planner_stages: Vec::new(),
        }
    }
}

/// Typed declaration of how a tool maps arguments to one or more effect targets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectTargetSpec {
    /// Equivalent argument names for a single logical target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// Argument names that jointly form a compound effect scope.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compound: Vec<String>,
}

/// Concrete targets covered by an effect intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectScope {
    pub targets: Vec<String>,
}

/// Canonical, typed description of the effect a tool call intends to perform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectIntent {
    pub target_key: String,
    pub effect_class: ToolSideEffectClass,
    pub scope: EffectScope,
    pub canonical_args_digest: ContentDigest,
}

impl EffectIntent {
    /// Digests the complete typed effect rather than any provider-specific JSON spelling.
    pub fn digest(&self) -> ContentDigest {
        digest_serializable(self)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub backend_kind: ToolBackendKind,
    pub input_schema: Value,
    pub output_mode: ToolOutputMode,
    pub read_only: bool,
    pub side_effect_class: ToolSideEffectClass,
    pub idempotency_class: ToolIdempotencyClass,
    pub approval_kind: ToolApprovalKind,
    pub timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_ttl_ms: Option<u64>,
    pub exposure_mode: ToolExposureMode,
    pub mcp_surface_kind: McpSurfaceKind,
    #[serde(default)]
    pub exposure_policy: ToolExposurePolicy,
    #[serde(default)]
    pub receipt_persistence: ToolReceiptPersistence,
    #[serde(default)]
    pub effect_target: EffectTargetSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_size_limit_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<Value>,
    #[serde(skip)]
    pub rollback_contract: Option<Arc<dyn CompensatingAction + Send + Sync>>,
}

impl std::fmt::Debug for ToolDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolDescriptor")
            .field("name", &self.name)
            .field("version", &self.version)
            .field("backend_kind", &self.backend_kind)
            .field("read_only", &self.read_only)
            .field("side_effect_class", &self.side_effect_class)
            .field("has_rollback", &self.rollback_contract.is_some())
            .finish()
    }
}

impl ToolDescriptor {
    /// Returns true when this descriptor can be dispatched through a local function bridge.
    pub fn supports_local_dispatch(&self) -> bool {
        matches!(
            self.backend_kind,
            ToolBackendKind::LocalFunction
                | ToolBackendKind::OpenAiFunction
                | ToolBackendKind::OllamaFunction
        )
    }

    /// Returns the stable digest identifying this tool method and version.
    pub fn method_digest(&self) -> ContentDigest {
        digest_serializable(&serde_json::json!({
            "name": self.name,
            "version": self.version,
        }))
    }

    /// Converts provider arguments into a typed, canonical effect intent.
    pub fn describe_effect(&self, args: &Value) -> Result<EffectIntent, ToolError> {
        let mut canonical_args = args.clone();
        let targets = if !self.effect_target.compound.is_empty() {
            self.effect_target
                .compound
                .iter()
                .map(|name| required_target(args, name))
                .collect::<Result<Vec<_>, _>>()?
        } else if !self.effect_target.aliases.is_empty() {
            let target = self
                .effect_target
                .aliases
                .iter()
                .find_map(|name| args.get(name).and_then(Value::as_str))
                .ok_or_else(|| {
                    ToolError::new(
                        ToolErrorClass::InvalidArguments,
                        "effect target is missing or is not a string",
                    )
                })?
                .to_owned();
            if let Value::Object(object) = &mut canonical_args {
                for alias in &self.effect_target.aliases {
                    object.remove(alias);
                }
                object.insert("$effect_target".into(), Value::String(target.clone()));
            }
            vec![target]
        } else {
            Vec::new()
        };

        let target_key = match targets.as_slice() {
            [] => self.name.clone(),
            [target] => target.clone(),
            _ => serde_json::to_string(&targets).unwrap_or_else(|_| "[]".into()),
        };
        Ok(EffectIntent {
            target_key,
            effect_class: self.side_effect_class.clone(),
            scope: EffectScope { targets },
            canonical_args_digest: digest_serializable(&canonical_args),
        })
    }
}

fn required_target(args: &Value, name: &str) -> Result<String, ToolError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            ToolError::new(
                ToolErrorClass::InvalidArguments,
                format!("compound effect target {name} is missing or is not a string"),
            )
        })
}

fn digest_serializable(value: &impl Serialize) -> ContentDigest {
    let mut builder = DigestBuilder::new();
    if let Ok(value) = serde_json::to_value(value) {
        let _ = builder.update_json(&value);
    }
    builder.finalize()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolBudgetContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_budget_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_budget_units: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCtx {
    pub trace_ctx: TraceCtx,
    pub attempt_id: AttemptId,
    pub trial_id: TrialId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workload_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_context: Option<ToolBudgetContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ScopeKey>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_grant: Option<ApprovalGrant>,
    #[serde(default, skip_serializing, skip_deserializing)]
    pub execution_permit: Option<Arc<ToolExecutionPermit>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    pub caller: String,
    pub planner_stage: ToolPlannerStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_receipt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family_receipt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_parent_receipt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_oracle_lease_id: Option<RemoteOracleLeaseId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_slice_result_id: Option<RemoteSliceResultId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attestation_envelope_id: Option<AttestationEnvelopeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_runtime_replay_ticket_id: Option<CrossRuntimeReplayTicketId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_owner: Option<ToolRetryOwner>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub descriptor_name: String,
    pub descriptor_version: String,
    pub arguments: Value,
    pub origin_kind: ToolOriginKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_call_id: Option<String>,
    pub tool_run_id: String,
}

impl ToolCall {
    /// Creates a tool call payload with a fresh tool-run identifier.
    pub fn new(
        descriptor_name: impl Into<String>,
        descriptor_version: impl Into<String>,
        arguments: Value,
        origin_kind: ToolOriginKind,
    ) -> Self {
        Self {
            descriptor_name: descriptor_name.into(),
            descriptor_version: descriptor_version.into(),
            arguments,
            origin_kind,
            provider_call_id: None,
            tool_run_id: uuid::Uuid::new_v4().to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolArtifactRef {
    pub artifact_id: ArtifactId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolJobHandle {
    pub job_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub mode: ToolOutputMode,
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_text: Option<String>,
}

impl ToolResult {
    /// Builds a structured-JSON tool result.
    pub fn json(payload: Value) -> Self {
        Self {
            mode: ToolOutputMode::StructuredJson,
            payload,
            display_text: None,
        }
    }

    /// Builds a text-mode tool result.
    pub fn text(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            mode: ToolOutputMode::Text,
            payload: Value::String(text.clone()),
            display_text: Some(text),
        }
    }

    /// Builds an artifact-reference tool result.
    pub fn artifact_refs(refs: Vec<ToolArtifactRef>) -> Self {
        Self {
            mode: ToolOutputMode::ArtifactRefs,
            payload: serde_json::to_value(refs).unwrap_or(Value::Array(Vec::new())),
            display_text: None,
        }
    }

    /// Builds a job-handle tool result.
    pub fn job_handle(handle: ToolJobHandle) -> Self {
        Self {
            mode: ToolOutputMode::JobHandle,
            payload: serde_json::to_value(handle).unwrap_or(Value::Null),
            display_text: None,
        }
    }

    /// Renders the result into the string form exposed back to the model.
    pub fn to_model_output(&self) -> String {
        match self.mode {
            ToolOutputMode::Text => self
                .display_text
                .clone()
                .unwrap_or_else(|| self.payload.as_str().unwrap_or_default().to_string()),
            ToolOutputMode::StructuredJson
            | ToolOutputMode::ArtifactRefs
            | ToolOutputMode::JobHandle => serde_json::to_string(&self.payload)
                .unwrap_or_else(|_| "\"<tool_result_unserializable>\"".to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Error)]
#[error("{class:?}: {message}")]
pub struct ToolError {
    pub class: ToolErrorClass,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl ToolError {
    /// Creates a non-retryable tool error with the supplied class and message.
    pub fn new(class: ToolErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
            retryable: false,
            details: None,
        }
    }
}

/// One offline-verifiable hop in the authority chain for an execution receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityLineageEntry {
    pub origin_class: String,
    pub principal: String,
    pub permit_id: String,
    pub policy_version: String,
}

/// Verifies that authority lineage is populated and covers the complete execution chain.
pub fn verify_authority_lineage(lineage: &[AuthorityLineageEntry]) -> Result<(), ToolError> {
    const REQUIRED: [&str; 5] = ["request", "policy", "approval", "permit", "effect"];
    if lineage.iter().any(|entry| {
        entry.origin_class.is_empty()
            || entry.principal.is_empty()
            || entry.permit_id.is_empty()
            || entry.policy_version.is_empty()
    }) {
        return Err(ToolError::new(
            ToolErrorClass::ProviderContract,
            "authority lineage contains an incomplete entry",
        ));
    }
    if REQUIRED
        .iter()
        .any(|required| !lineage.iter().any(|entry| entry.origin_class == *required))
    {
        return Err(ToolError::new(
            ToolErrorClass::ProviderContract,
            "authority lineage does not cover request, policy, approval, permit, and effect",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolReceiptPhase {
    Preflight,
    #[default]
    Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolReceiptResolution {
    Pending,
    #[default]
    Resolved,
    Unresolved,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolReceipt {
    pub receipt_id: String,
    pub tool_name: String,
    pub tool_version: String,
    pub backend_kind: ToolBackendKind,
    pub input_digest: ContentDigest,
    pub output_digest_or_refs: Value,
    pub policy_hash: ContentDigest,
    pub approval_state: ToolApprovalState,
    #[serde(default)]
    pub phase: ToolReceiptPhase,
    #[serde(default)]
    pub resolution: ToolReceiptResolution,
    #[serde(default)]
    pub authority_lineage: Vec<AuthorityLineageEntry>,
    pub host_identity: String,
    pub started_at: String,
    pub finished_at: String,
    pub trace_ctx: TraceCtx,
    pub attempt_id: AttemptId,
    pub trial_id: TrialId,
    pub planner_stage: ToolPlannerStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workload_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_context: Option<ToolBudgetContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_receipt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight_receipt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family_receipt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_parent_receipt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_oracle_lease_id: Option<RemoteOracleLeaseId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_slice_result_id: Option<RemoteSliceResultId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attestation_envelope_id: Option<AttestationEnvelopeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_runtime_replay_ticket_id: Option<CrossRuntimeReplayTicketId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<ToolErrorClass>,
    pub retry_owner: ToolRetryOwner,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_link: Option<String>,
    pub tool_run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_call_id: Option<String>,
}

impl ToolReceipt {
    /// Verifies the embedded request-to-effect authority chain offline.
    pub fn verify_authority_lineage(&self) -> Result<(), ToolError> {
        verify_authority_lineage(&self.authority_lineage)
    }

    /// Normalizes a runtime-native tool receipt into the canonical Forge receipt schema.
    pub fn to_forge_tool_receipt_v2(
        &self,
        raw_payload: Value,
    ) -> semantic_memory_forge::ForgeToolReceiptV2 {
        semantic_memory_forge::ForgeToolReceiptV2 {
            schema_version: semantic_memory_forge::FORGE_TOOL_RECEIPT_V2_SCHEMA.into(),
            receipt_id: self.receipt_id.clone(),
            tool_run_id: self.tool_run_id.clone(),
            tool_name: self.tool_name.clone(),
            tool_version: self.tool_version.clone(),
            backend_kind: self.backend_kind.as_str().into(),
            input_digest: self.input_digest.clone(),
            output_digest_or_refs: self.output_digest_or_refs.clone(),
            policy_hash: self.policy_hash.clone(),
            approval_state: self.approval_state.as_str().into(),
            host_identity: self.host_identity.clone(),
            started_at: self.started_at.clone(),
            finished_at: self.finished_at.clone(),
            trace_ctx: self.trace_ctx.clone(),
            attempt_id: self.attempt_id.clone(),
            trial_id: self.trial_id.clone(),
            planner_stage: self.planner_stage.as_str().into(),
            deadline: self.deadline.clone(),
            workload_class: self.workload_class.clone(),
            budget_context: self.budget_context.as_ref().map(|budget| {
                semantic_memory_forge::ForgeToolBudgetContext {
                    budget_kind: budget.budget_kind.clone(),
                    max_steps: budget.max_steps,
                    time_budget_ms: budget.time_budget_ms,
                    cost_budget_units: budget.cost_budget_units,
                }
            }),
            parent_receipt_id: self.parent_receipt_id.clone(),
            family_receipt_id: self.family_receipt_id.clone(),
            replay_parent_receipt_id: self.replay_parent_receipt_id.clone(),
            error_class: self.error_class.as_ref().map(|class| format!("{class:?}")),
            retry_owner: self.retry_owner.as_str().into(),
            replay_link: self.replay_link.clone(),
            provider_call_id: self.provider_call_id.clone(),
            raw_payload,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolEffectDispatchReceiptV1 {
    pub schema_version: String,
    pub tool_effect_dispatch_receipt_id: ToolEffectDispatchReceiptId,
    pub effect_commit_decision_id: EffectCommitDecisionId,
    pub tool_receipt_id: String,
    pub provider_route: Vec<String>,
    pub dispatch_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_execution_receipt_id: Option<EffectExecutionReceiptId>,
    pub deadline_at_dispatch: String,
    pub cancellation_reason: String,
}
