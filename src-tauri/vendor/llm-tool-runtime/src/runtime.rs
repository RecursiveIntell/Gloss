pub use crate::ToolError as ToolRuntimeError;
use crate::{
    validate_arguments_against_schema, ApprovalGrantEffectClass, ToolApprovalKind,
    ToolApprovalState, ToolCall, ToolDescriptor, ToolError, ToolErrorClass, ToolExecutionPermit,
    ToolReceipt, ToolReceiptPersistence, ToolRegistry, ToolResult, ToolRetryOwner,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use stack_ids::{ContentDigest, DigestBuilder};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[async_trait]
pub trait ApprovalPolicy: Send + Sync {
    async fn evaluate(
        &self,
        descriptor: &ToolDescriptor,
        ctx: &crate::ToolCtx,
        call: &ToolCall,
    ) -> Result<ToolApprovalState, ToolError>;
}

#[derive(Default)]
pub struct StaticApprovalPolicy;

#[async_trait]
impl ApprovalPolicy for StaticApprovalPolicy {
    async fn evaluate(
        &self,
        descriptor: &ToolDescriptor,
        ctx: &crate::ToolCtx,
        call: &ToolCall,
    ) -> Result<ToolApprovalState, ToolError> {
        if descriptor_has_tokenless_read_only_path(descriptor) {
            return Ok(ToolApprovalState::NotRequired);
        }

        let permit = ctx.execution_permit.as_ref().ok_or_else(|| {
            ToolError::new(
                ToolErrorClass::ApprovalRequired,
                format!("tool {} requires an execution permit", descriptor.name),
            )
        })?;
        if !execution_permit_authorizes(permit, ctx, call) {
            return Err(ToolError::new(
                ToolErrorClass::Denied,
                format!("execution permit does not cover tool {}", descriptor.name),
            ));
        }

        if descriptor.approval_kind == ToolApprovalKind::None {
            return Ok(ToolApprovalState::NotRequired);
        }

        if let Some(grant) = ctx.approval_grant.as_ref() {
            if approval_grant_authorizes(grant, descriptor, ctx, call, &Utc::now().to_rfc3339()) {
                return Ok(ToolApprovalState::Approved);
            }
            return Err(ToolError::new(
                ToolErrorClass::Denied,
                format!("approval grant does not cover tool {}", descriptor.name),
            ));
        }

        Err(ToolError::new(
            ToolErrorClass::ApprovalRequired,
            format!("tool {} requires approval", descriptor.name),
        ))
    }
}

fn descriptor_has_tokenless_read_only_path(descriptor: &ToolDescriptor) -> bool {
    descriptor.read_only
        && matches!(
            descriptor.side_effect_class,
            crate::ToolSideEffectClass::ReadOnly
        )
}

fn execution_permit_authorizes(
    permit: &ToolExecutionPermit,
    ctx: &crate::ToolCtx,
    call: &ToolCall,
) -> bool {
    let Some(scope) = ctx.scope.as_ref() else {
        return false;
    };
    if permit.scope().namespace() != scope.namespace {
        return false;
    }
    let Some(target_key) = extract_target_key(call) else {
        return false;
    };
    permit.scope().target_key() == target_key
}

fn approval_grant_authorizes(
    grant: &crate::ApprovalGrant,
    descriptor: &ToolDescriptor,
    ctx: &crate::ToolCtx,
    call: &ToolCall,
    evaluated_at: &str,
) -> bool {
    if grant.approver_lineage.is_empty() {
        return false;
    }
    if grant
        .expires_at
        .as_ref()
        .is_some_and(|expires_at| expires_at.as_str() <= evaluated_at)
    {
        return false;
    }
    if grant.scope.tool_name != descriptor.name {
        return false;
    }
    if grant
        .scope
        .planner_stage
        .as_ref()
        .is_some_and(|planner_stage| planner_stage != &ctx.planner_stage)
    {
        return false;
    }

    let Some(required_effect_class) =
        ApprovalGrantEffectClass::for_tool_side_effect_class(&descriptor.side_effect_class)
    else {
        return false;
    };
    if grant.scope.effect_class != required_effect_class {
        return false;
    }

    let Some(scope) = ctx.scope.as_ref() else {
        return false;
    };
    if grant.scope.namespace != scope.namespace {
        return false;
    }
    if grant
        .scope
        .target_key
        .as_ref()
        .is_some_and(|target_key| Some(target_key.as_str()) != extract_target_key(call))
    {
        return false;
    }
    if let Some(permit) = ctx.execution_permit.as_ref() {
        if grant
            .decision_id
            .as_ref()
            .is_some_and(|decision_id| decision_id != permit.decision_id())
        {
            return false;
        }
        if grant
            .approval_record_id
            .as_ref()
            .is_some_and(|approval_record_id| {
                permit.approval_record_id() != Some(approval_record_id)
            })
        {
            return false;
        }
    }

    true
}

fn extract_target_key(call: &ToolCall) -> Option<&str> {
    ["target", "artifact_id", "candidate_id", "task_id"]
        .into_iter()
        .find_map(|key| call.arguments.get(key).and_then(|value| value.as_str()))
}

#[async_trait]
pub trait ToolReceiptSink: Send + Sync {
    async fn persist(&self, receipt: &ToolReceipt) -> Result<(), ToolError>;
}

#[derive(Default)]
pub struct InMemoryReceiptSink {
    receipts: std::sync::Mutex<Vec<ToolReceipt>>,
}

impl InMemoryReceiptSink {
    /// Returns a clone of the receipts persisted into this in-memory sink.
    pub fn receipts(&self) -> Vec<ToolReceipt> {
        self.receipts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

#[async_trait]
impl ToolReceiptSink for InMemoryReceiptSink {
    async fn persist(&self, receipt: &ToolReceipt) -> Result<(), ToolError> {
        self.receipts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(receipt.clone());
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ToolRuntimeConfig {
    pub default_timeout_ms: u64,
    pub default_output_size_limit_bytes: usize,
}

impl Default for ToolRuntimeConfig {
    fn default() -> Self {
        Self {
            default_timeout_ms: 30_000,
            default_output_size_limit_bytes: 256 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolExecution {
    pub result: Result<ToolResult, ToolError>,
    pub receipt: ToolReceipt,
}

pub struct ToolRuntime {
    registry: ToolRegistry,
    approval_policy: Arc<dyn ApprovalPolicy>,
    receipt_sink: Option<Arc<dyn ToolReceiptSink>>,
    config: ToolRuntimeConfig,
}

impl ToolRuntime {
    /// Creates a runtime around the supplied registry using default policy and limits.
    pub fn new(registry: ToolRegistry) -> Self {
        Self {
            registry,
            approval_policy: Arc::new(StaticApprovalPolicy),
            receipt_sink: None,
            config: ToolRuntimeConfig::default(),
        }
    }

    /// Overrides runtime-wide timeout and output-size defaults.
    pub fn with_config(mut self, config: ToolRuntimeConfig) -> Self {
        self.config = config;
        self
    }

    /// Replaces the approval policy used before dispatching tools.
    pub fn with_approval_policy(mut self, approval_policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = approval_policy;
        self
    }

    /// Installs the durable receipt sink used for ForgeRaw receipt persistence.
    pub fn with_receipt_sink(mut self, receipt_sink: Arc<dyn ToolReceiptSink>) -> Self {
        self.receipt_sink = Some(receipt_sink);
        self
    }

    /// Borrows the underlying tool registry.
    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    pub async fn execute(
        &self,
        ctx: &crate::ToolCtx,
        call: &ToolCall,
        permit: Option<&ToolExecutionPermit>,
        cancel: Option<&AtomicBool>,
    ) -> ToolExecution {
        let started_at = Utc::now();
        let mut execution_ctx = ctx.clone();
        execution_ctx.execution_permit = permit.cloned();
        let descriptor = match self.registry.get(&call.descriptor_name) {
            Some(tool) => tool.descriptor().clone(),
            None => {
                let error = ToolError::new(
                    ToolErrorClass::UnknownTool,
                    format!("unknown tool {}", call.descriptor_name),
                );
                let receipt = self.failure_receipt(ctx, call, started_at, error.clone(), None);
                return ToolExecution {
                    result: Err(error),
                    receipt,
                };
            }
        };

        if descriptor.version != call.descriptor_version {
            let error = ToolError::new(
                ToolErrorClass::ProviderContract,
                format!(
                    "tool version mismatch for {}: expected {}, got {}",
                    descriptor.name, descriptor.version, call.descriptor_version
                ),
            );
            let receipt =
                self.failure_receipt(ctx, call, started_at, error.clone(), Some(&descriptor));
            return ToolExecution {
                result: Err(error),
                receipt,
            };
        }

        if let Err(error) =
            validate_arguments_against_schema(&descriptor.input_schema, &call.arguments)
        {
            let receipt =
                self.failure_receipt(ctx, call, started_at, error.clone(), Some(&descriptor));
            return ToolExecution {
                result: Err(error),
                receipt,
            };
        }

        let approval_state = match self
            .approval_policy
            .evaluate(&descriptor, &execution_ctx, call)
            .await
        {
            Ok(state) => state,
            Err(error) => {
                let receipt =
                    self.failure_receipt(ctx, call, started_at, error.clone(), Some(&descriptor));
                return ToolExecution {
                    result: Err(error),
                    receipt,
                };
            }
        };

        if matches!(approval_state, ToolApprovalState::Denied) {
            let error = ToolError::new(
                ToolErrorClass::Denied,
                format!("tool {} was denied by approval policy", descriptor.name),
            );
            let receipt =
                self.failure_receipt(ctx, call, started_at, error.clone(), Some(&descriptor));
            return ToolExecution {
                result: Err(error),
                receipt,
            };
        }

        let Some(tool) = self.registry.get(&call.descriptor_name) else {
            let error = ToolError::new(
                ToolErrorClass::UnknownTool,
                format!("tool {} disappeared before execution", call.descriptor_name),
            );
            let receipt =
                self.failure_receipt(ctx, call, started_at, error.clone(), Some(&descriptor));
            return ToolExecution {
                result: Err(error),
                receipt,
            };
        };
        let timeout_ms = resolve_timeout_ms(&descriptor, ctx, self.config.default_timeout_ms);
        let fut = async {
            if let Some(flag) = cancel {
                if flag.load(Ordering::Relaxed) {
                    return Err(ToolError::new(ToolErrorClass::Cancelled, "tool cancelled"));
                }
            }

            let execute = tool.invoke(&execution_ctx, call);
            match cancel {
                Some(flag) => {
                    tokio::select! {
                        result = execute => result,
                        _ = wait_for_cancel(flag) => Err(ToolError::new(ToolErrorClass::Cancelled, "tool cancelled")),
                    }
                }
                None => execute.await,
            }
        };

        let result = match tokio::time::timeout(Duration::from_millis(timeout_ms), fut).await {
            Ok(inner) => inner,
            Err(_) => Err(ToolError::new(
                ToolErrorClass::Timeout,
                format!("tool {} timed out after {}ms", descriptor.name, timeout_ms),
            )),
        };

        let result = match result {
            Ok(tool_result) => {
                if let Err(error) =
                    enforce_output_size_limit(&descriptor, &tool_result, &self.config)
                {
                    Err(error)
                } else {
                    Ok(tool_result)
                }
            }
            Err(error) => Err(error),
        };

        let receipt = match &result {
            Ok(tool_result) => self.success_receipt(
                ctx,
                call,
                &descriptor,
                started_at,
                tool_result,
                approval_state.clone(),
            ),
            Err(error) => self.failure_receipt_with_approval(
                ctx,
                call,
                started_at,
                error.clone(),
                &descriptor,
                approval_state.clone(),
            ),
        };

        if descriptor.receipt_persistence == ToolReceiptPersistence::ForgeRaw {
            if let Some(sink) = &self.receipt_sink {
                if let Err(error) = sink.persist(&receipt).await {
                    let receipt = self.failure_receipt_with_approval(
                        ctx,
                        call,
                        started_at,
                        error.clone(),
                        &descriptor,
                        approval_state,
                    );
                    return ToolExecution {
                        result: Err(error),
                        receipt,
                    };
                }
            } else {
                let error = ToolError::new(
                    ToolErrorClass::ReceiptPersistence,
                    format!("tool {} requires a durable receipt sink", descriptor.name),
                );
                let receipt = self.failure_receipt_with_approval(
                    ctx,
                    call,
                    started_at,
                    error.clone(),
                    &descriptor,
                    approval_state,
                );
                return ToolExecution {
                    result: Err(error),
                    receipt,
                };
            }
        }

        ToolExecution { result, receipt }
    }

    fn success_receipt(
        &self,
        ctx: &crate::ToolCtx,
        call: &ToolCall,
        descriptor: &ToolDescriptor,
        started_at: DateTime<Utc>,
        result: &ToolResult,
        approval_state: ToolApprovalState,
    ) -> ToolReceipt {
        ToolReceipt {
            receipt_id: uuid::Uuid::new_v4().to_string(),
            tool_name: descriptor.name.clone(),
            tool_version: descriptor.version.clone(),
            backend_kind: descriptor.backend_kind.clone(),
            input_digest: digest_json(&call.arguments),
            output_digest_or_refs: output_digest_or_refs(result),
            policy_hash: policy_hash(descriptor),
            approval_state,
            host_identity: ctx.caller.clone(),
            started_at: started_at.to_rfc3339(),
            finished_at: Utc::now().to_rfc3339(),
            trace_ctx: ctx.trace_ctx.clone(),
            attempt_id: ctx.attempt_id.clone(),
            trial_id: ctx.trial_id.clone(),
            planner_stage: ctx.planner_stage.clone(),
            deadline: ctx.deadline.clone(),
            workload_class: ctx.workload_class.clone(),
            budget_context: ctx.budget_context.clone(),
            parent_receipt_id: ctx.parent_receipt_id.clone(),
            family_receipt_id: ctx.family_receipt_id.clone(),
            replay_parent_receipt_id: ctx.replay_parent_receipt_id.clone(),
            remote_oracle_lease_id: ctx.remote_oracle_lease_id.clone(),
            remote_slice_result_id: ctx.remote_slice_result_id.clone(),
            attestation_envelope_id: ctx.attestation_envelope_id.clone(),
            cross_runtime_replay_ticket_id: ctx.cross_runtime_replay_ticket_id.clone(),
            error_class: None,
            retry_owner: ctx.retry_owner.clone().unwrap_or(ToolRetryOwner::External),
            replay_link: Some(format!("tool_run:{}", call.tool_run_id)),
            tool_run_id: call.tool_run_id.clone(),
            provider_call_id: call.provider_call_id.clone(),
        }
    }

    fn failure_receipt(
        &self,
        ctx: &crate::ToolCtx,
        call: &ToolCall,
        started_at: DateTime<Utc>,
        error: ToolError,
        descriptor: Option<&ToolDescriptor>,
    ) -> ToolReceipt {
        let descriptor_backend_kind = descriptor
            .map(|desc| desc.backend_kind.clone())
            .unwrap_or(crate::ToolBackendKind::LocalFunction);
        let tool_version = descriptor
            .map(|desc| desc.version.clone())
            .unwrap_or_else(|| call.descriptor_version.clone());
        let policy_hash = descriptor
            .map(policy_hash)
            .unwrap_or_else(|| ContentDigest::compute(b"missing_policy"));

        ToolReceipt {
            receipt_id: uuid::Uuid::new_v4().to_string(),
            tool_name: call.descriptor_name.clone(),
            tool_version,
            backend_kind: descriptor_backend_kind,
            input_digest: digest_json(&call.arguments),
            output_digest_or_refs: serde_json::json!({"error": error.message}),
            policy_hash,
            approval_state: ToolApprovalState::Denied,
            host_identity: ctx.caller.clone(),
            started_at: started_at.to_rfc3339(),
            finished_at: Utc::now().to_rfc3339(),
            trace_ctx: ctx.trace_ctx.clone(),
            attempt_id: ctx.attempt_id.clone(),
            trial_id: ctx.trial_id.clone(),
            planner_stage: ctx.planner_stage.clone(),
            deadline: ctx.deadline.clone(),
            workload_class: ctx.workload_class.clone(),
            budget_context: ctx.budget_context.clone(),
            parent_receipt_id: ctx.parent_receipt_id.clone(),
            family_receipt_id: ctx.family_receipt_id.clone(),
            replay_parent_receipt_id: ctx.replay_parent_receipt_id.clone(),
            remote_oracle_lease_id: ctx.remote_oracle_lease_id.clone(),
            remote_slice_result_id: ctx.remote_slice_result_id.clone(),
            attestation_envelope_id: ctx.attestation_envelope_id.clone(),
            cross_runtime_replay_ticket_id: ctx.cross_runtime_replay_ticket_id.clone(),
            error_class: Some(error.class),
            retry_owner: ctx.retry_owner.clone().unwrap_or(ToolRetryOwner::External),
            replay_link: Some(format!("tool_run:{}", call.tool_run_id)),
            tool_run_id: call.tool_run_id.clone(),
            provider_call_id: call.provider_call_id.clone(),
        }
    }

    fn failure_receipt_with_approval(
        &self,
        ctx: &crate::ToolCtx,
        call: &ToolCall,
        started_at: DateTime<Utc>,
        error: ToolError,
        descriptor: &ToolDescriptor,
        approval_state: ToolApprovalState,
    ) -> ToolReceipt {
        ToolReceipt {
            receipt_id: uuid::Uuid::new_v4().to_string(),
            tool_name: descriptor.name.clone(),
            tool_version: descriptor.version.clone(),
            backend_kind: descriptor.backend_kind.clone(),
            input_digest: digest_json(&call.arguments),
            output_digest_or_refs: serde_json::json!({"error": error.message}),
            policy_hash: policy_hash(descriptor),
            approval_state,
            host_identity: ctx.caller.clone(),
            started_at: started_at.to_rfc3339(),
            finished_at: Utc::now().to_rfc3339(),
            trace_ctx: ctx.trace_ctx.clone(),
            attempt_id: ctx.attempt_id.clone(),
            trial_id: ctx.trial_id.clone(),
            planner_stage: ctx.planner_stage.clone(),
            deadline: ctx.deadline.clone(),
            workload_class: ctx.workload_class.clone(),
            budget_context: ctx.budget_context.clone(),
            parent_receipt_id: ctx.parent_receipt_id.clone(),
            family_receipt_id: ctx.family_receipt_id.clone(),
            replay_parent_receipt_id: ctx.replay_parent_receipt_id.clone(),
            remote_oracle_lease_id: ctx.remote_oracle_lease_id.clone(),
            remote_slice_result_id: ctx.remote_slice_result_id.clone(),
            attestation_envelope_id: ctx.attestation_envelope_id.clone(),
            cross_runtime_replay_ticket_id: ctx.cross_runtime_replay_ticket_id.clone(),
            error_class: Some(error.class),
            retry_owner: ctx.retry_owner.clone().unwrap_or(ToolRetryOwner::External),
            replay_link: Some(format!("tool_run:{}", call.tool_run_id)),
            tool_run_id: call.tool_run_id.clone(),
            provider_call_id: call.provider_call_id.clone(),
        }
    }
}

fn resolve_timeout_ms(
    descriptor: &ToolDescriptor,
    ctx: &crate::ToolCtx,
    default_timeout_ms: u64,
) -> u64 {
    let mut timeout_ms = if descriptor.timeout_ms == 0 {
        default_timeout_ms
    } else {
        descriptor.timeout_ms
    };

    if let Some(deadline) = &ctx.deadline {
        if let Ok(deadline_at) = DateTime::parse_from_rfc3339(deadline) {
            let remaining = deadline_at
                .with_timezone(&Utc)
                .signed_duration_since(Utc::now())
                .num_milliseconds()
                .max(1) as u64;
            timeout_ms = timeout_ms.min(remaining);
        }
    }

    timeout_ms
}

fn enforce_output_size_limit(
    descriptor: &ToolDescriptor,
    result: &ToolResult,
    config: &ToolRuntimeConfig,
) -> Result<(), ToolError> {
    let output_size_limit = descriptor
        .output_size_limit_bytes
        .unwrap_or(config.default_output_size_limit_bytes);
    let output_json = serde_json::to_vec(result).unwrap_or_default();

    if output_json.len() > output_size_limit {
        return Err(ToolError::new(
            ToolErrorClass::OutputTooLarge,
            format!(
                "tool {} output exceeded {} bytes",
                descriptor.name, output_size_limit
            ),
        ));
    }

    Ok(())
}

fn digest_json(value: &serde_json::Value) -> ContentDigest {
    let mut builder = DigestBuilder::new();
    if let Err(e) = builder.update_json(value) {
        tracing::warn!(error = %e, "digest_json: update_json failed; finalizing partial digest");
    }
    builder.finalize()
}

fn policy_hash(descriptor: &ToolDescriptor) -> ContentDigest {
    let payload = serde_json::json!({
        "read_only": descriptor.read_only,
        "side_effect_class": descriptor.side_effect_class,
        "idempotency_class": descriptor.idempotency_class,
        "approval_kind": descriptor.approval_kind,
        "exposure_mode": descriptor.exposure_mode,
        "receipt_persistence": descriptor.receipt_persistence,
    });
    digest_json(&payload)
}

fn output_digest_or_refs(result: &ToolResult) -> serde_json::Value {
    match result.mode {
        crate::ToolOutputMode::ArtifactRefs => serde_json::json!({
            "artifacts": result.payload,
        }),
        _ => serde_json::json!({
            "digest": digest_json(&result.payload).hex().to_string(),
        }),
    }
}

async fn wait_for_cancel(flag: &AtomicBool) {
    while !flag.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Tool, ToolApprovalKind, ToolBackendKind, ToolBudgetContext, ToolCall, ToolCtx,
        ToolDescriptor, ToolExposureMode, ToolExposurePolicy, ToolIdempotencyClass, ToolOriginKind,
        ToolOutputMode, ToolPlannerStage, ToolReceiptPersistence, ToolResult, ToolSideEffectClass,
    };
    use async_trait::async_trait;
    use semantic_memory_forge::FORGE_TOOL_RECEIPT_V2_SCHEMA;
    use serde_json::json;
    use stack_ids::{AttemptId, TraceCtx, TrialId};

    #[derive(Clone)]
    struct EchoTool {
        descriptor: ToolDescriptor,
    }

    struct SlowTool {
        descriptor: ToolDescriptor,
        delay_ms: u64,
    }

    #[async_trait]
    impl Tool for EchoTool {
        fn descriptor(&self) -> &ToolDescriptor {
            &self.descriptor
        }

        async fn invoke(&self, _ctx: &ToolCtx, call: &ToolCall) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::json(call.arguments.clone()))
        }
    }

    #[async_trait]
    impl Tool for SlowTool {
        fn descriptor(&self) -> &ToolDescriptor {
            &self.descriptor
        }

        async fn invoke(&self, _ctx: &ToolCtx, _call: &ToolCall) -> Result<ToolResult, ToolError> {
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            Ok(ToolResult::json(json!({"message": "slow"})))
        }
    }

    struct RecordingSink(std::sync::Mutex<Vec<ToolReceipt>>);

    #[async_trait]
    impl ToolReceiptSink for RecordingSink {
        async fn persist(&self, receipt: &ToolReceipt) -> Result<(), ToolError> {
            self.0.lock().unwrap().push(receipt.clone());
            Ok(())
        }
    }

    fn echo_descriptor() -> ToolDescriptor {
        ToolDescriptor {
            name: "echo".into(),
            version: "1.0.0".into(),
            description: Some("Echo the input back".into()),
            backend_kind: ToolBackendKind::LocalFunction,
            input_schema: json!({
                "type": "object",
                "required": ["message"],
                "properties": {"message": {"type": "string"}},
                "additionalProperties": false
            }),
            output_mode: ToolOutputMode::StructuredJson,
            read_only: true,
            side_effect_class: ToolSideEffectClass::ReadOnly,
            idempotency_class: ToolIdempotencyClass::Idempotent,
            approval_kind: ToolApprovalKind::None,
            timeout_ms: 1_000,
            concurrency_key: None,
            cache_ttl_ms: None,
            exposure_mode: ToolExposureMode::Auto,
            mcp_surface_kind: crate::McpSurfaceKind::Tool,
            exposure_policy: ToolExposurePolicy {
                max_calls_per_turn: 1,
                ..Default::default()
            },
            receipt_persistence: ToolReceiptPersistence::ForgeRaw,
            output_size_limit_bytes: None,
            provider_payload: None,
        }
    }

    fn tool_ctx() -> ToolCtx {
        ToolCtx {
            trace_ctx: TraceCtx::generate(),
            attempt_id: AttemptId::generate(),
            trial_id: TrialId::generate(),
            deadline: None,
            workload_class: None,
            budget_context: None,
            scope: None,
            dry_run: false,
            approval_grant: None,
            execution_permit: None,
            idempotency_key: None,
            caller: "test-host".into(),
            planner_stage: ToolPlannerStage::Execution,
            parent_receipt_id: None,
            family_receipt_id: None,
            replay_parent_receipt_id: None,
            remote_oracle_lease_id: None,
            remote_slice_result_id: None,
            attestation_envelope_id: None,
            cross_runtime_replay_ticket_id: None,
            retry_owner: Some(ToolRetryOwner::LlmPipeline),
        }
    }

    fn execution_permit_for(target_key: &str) -> crate::ToolExecutionPermit {
        crate::ToolExecutionPermit::new(
            stack_ids::ExecutionPermitId::generate(),
            stack_ids::PolicyDecisionId::generate(),
            None,
            "runtime-tests",
            target_key,
        )
    }

    fn approval_grant_for(
        tool_name: &str,
        effect_class: crate::ApprovalGrantEffectClass,
        namespace: &str,
        target_key: Option<&str>,
    ) -> crate::ApprovalGrant {
        crate::ApprovalGrant::new(
            "policy-1",
            crate::ApprovalGrantScope {
                namespace: namespace.into(),
                target_key: target_key.map(str::to_owned),
                tool_name: tool_name.into(),
                effect_class,
                planner_stage: Some(ToolPlannerStage::Execution),
            },
            vec!["operator".into()],
            "2026-03-12T00:00:00Z",
            None,
            crate::ApprovalGrantCitation {
                applicability_context_id: stack_ids::ApplicabilityContextId::new(
                    "applicability-context-1",
                ),
                profile_set_id: stack_ids::ProfileSetId::new("profile-set-1"),
                composition_receipt_id: stack_ids::CompositionReceiptId::new(
                    "composition-receipt-1",
                ),
                effective_constitution_id: stack_ids::EffectiveConstitutionId::new(
                    "effective-constitution-1",
                ),
                compiled_obligation_set_id: stack_ids::CompiledObligationSetId::new(
                    "compiled-obligation-set-1",
                ),
            },
        )
    }

    #[tokio::test]
    async fn runtime_dispatches_and_persists_receipts() {
        let mut registry = ToolRegistry::new();
        registry.register(EchoTool {
            descriptor: echo_descriptor(),
        });
        let sink = Arc::new(RecordingSink(std::sync::Mutex::new(Vec::new())));
        let runtime = ToolRuntime::new(registry).with_receipt_sink(sink.clone());
        let call = ToolCall::new(
            "echo",
            "1.0.0",
            json!({"message": "hello"}),
            ToolOriginKind::Test,
        );

        let execution = runtime.execute(&tool_ctx(), &call, None, None).await;
        let value = execution.result.unwrap().payload;
        assert_eq!(value["message"], "hello");
        assert!(execution.receipt.error_class.is_none());
        assert_eq!(sink.0.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn runtime_returns_structured_unknown_tool_failure() {
        let runtime = ToolRuntime::new(ToolRegistry::new());
        let call = ToolCall::new("missing", "1.0.0", json!({}), ToolOriginKind::Test);

        let execution = runtime.execute(&tool_ctx(), &call, None, None).await;
        let error = execution.result.unwrap_err();

        assert_eq!(error.class, ToolErrorClass::UnknownTool);
        assert_eq!(
            execution.receipt.error_class,
            Some(ToolErrorClass::UnknownTool)
        );
    }

    #[tokio::test]
    async fn runtime_returns_structured_timeout_failure() {
        let mut registry = ToolRegistry::new();
        let mut descriptor = echo_descriptor();
        descriptor.name = "slow".into();
        descriptor.timeout_ms = 1;
        descriptor.receipt_persistence = ToolReceiptPersistence::Ephemeral;
        registry.register(SlowTool {
            descriptor,
            delay_ms: 20,
        });
        let runtime = ToolRuntime::new(registry);
        let call = ToolCall::new(
            "slow",
            "1.0.0",
            json!({"message": "slow"}),
            ToolOriginKind::Test,
        );

        let execution = runtime.execute(&tool_ctx(), &call, None, None).await;
        let error = execution.result.unwrap_err();

        assert_eq!(error.class, ToolErrorClass::Timeout);
        assert_eq!(execution.receipt.error_class, Some(ToolErrorClass::Timeout));
    }

    #[tokio::test]
    async fn runtime_returns_structured_cancelled_failure() {
        let mut registry = ToolRegistry::new();
        let mut descriptor = echo_descriptor();
        descriptor.name = "slow".into();
        descriptor.receipt_persistence = ToolReceiptPersistence::Ephemeral;
        registry.register(SlowTool {
            descriptor,
            delay_ms: 20,
        });
        let runtime = ToolRuntime::new(registry);
        let call = ToolCall::new(
            "slow",
            "1.0.0",
            json!({"message": "slow"}),
            ToolOriginKind::Test,
        );
        let cancel = AtomicBool::new(true);

        let execution = runtime
            .execute(&tool_ctx(), &call, None, Some(&cancel))
            .await;
        let error = execution.result.unwrap_err();

        assert_eq!(error.class, ToolErrorClass::Cancelled);
        assert_eq!(
            execution.receipt.error_class,
            Some(ToolErrorClass::Cancelled)
        );
    }

    #[tokio::test]
    async fn runtime_returns_structured_receipt_persistence_failure() {
        let mut registry = ToolRegistry::new();
        registry.register(EchoTool {
            descriptor: echo_descriptor(),
        });
        let runtime = ToolRuntime::new(registry);
        let call = ToolCall::new(
            "echo",
            "1.0.0",
            json!({"message": "hello"}),
            ToolOriginKind::Test,
        );

        let execution = runtime.execute(&tool_ctx(), &call, None, None).await;
        let error = execution.result.unwrap_err();

        assert_eq!(error.class, ToolErrorClass::ReceiptPersistence);
        assert_eq!(
            execution.receipt.error_class,
            Some(ToolErrorClass::ReceiptPersistence)
        );
    }

    #[tokio::test]
    async fn runtime_receipt_carries_budget_and_lineage_context() {
        let mut registry = ToolRegistry::new();
        registry.register(EchoTool {
            descriptor: echo_descriptor(),
        });
        let sink = Arc::new(RecordingSink(std::sync::Mutex::new(Vec::new())));
        let runtime = ToolRuntime::new(registry).with_receipt_sink(sink.clone());
        let call = ToolCall::new(
            "echo",
            "1.0.0",
            json!({"message": "lineage"}),
            ToolOriginKind::Test,
        );
        let mut ctx = tool_ctx();
        ctx.deadline = Some("2026-03-12T00:00:10Z".into());
        ctx.workload_class = Some("verification".into());
        ctx.budget_context = Some(ToolBudgetContext {
            budget_kind: Some("control_plane".into()),
            max_steps: Some(3),
            time_budget_ms: Some(750),
            cost_budget_units: Some(12),
        });
        ctx.parent_receipt_id = Some("dispatch-1".into());
        ctx.family_receipt_id = Some("attempt-family-1".into());
        ctx.replay_parent_receipt_id = Some("retry-0".into());
        ctx.remote_oracle_lease_id = Some(stack_ids::RemoteOracleLeaseId::new("lease-1"));
        ctx.remote_slice_result_id = Some(stack_ids::RemoteSliceResultId::new("result-1"));
        ctx.attestation_envelope_id = Some(stack_ids::AttestationEnvelopeId::new("attestation-1"));
        ctx.cross_runtime_replay_ticket_id = Some(stack_ids::CrossRuntimeReplayTicketId::new(
            "cross-runtime-replay-ticket-1",
        ));
        ctx.planner_stage = ToolPlannerStage::Audit;

        let execution = runtime.execute(&ctx, &call, None, None).await;
        assert!(execution.result.is_ok());
        assert_eq!(
            execution.receipt.deadline.as_deref(),
            Some("2026-03-12T00:00:10Z")
        );
        assert_eq!(
            execution.receipt.workload_class.as_deref(),
            Some("verification")
        );
        assert_eq!(
            execution
                .receipt
                .budget_context
                .as_ref()
                .and_then(|budget| budget.max_steps),
            Some(3)
        );
        assert_eq!(
            execution.receipt.parent_receipt_id.as_deref(),
            Some("dispatch-1")
        );
        assert_eq!(
            execution.receipt.family_receipt_id.as_deref(),
            Some("attempt-family-1")
        );
        assert_eq!(
            execution.receipt.replay_parent_receipt_id.as_deref(),
            Some("retry-0")
        );
        assert_eq!(
            execution
                .receipt
                .remote_oracle_lease_id
                .as_ref()
                .map(|id| id.as_str()),
            Some("lease-1")
        );
        assert_eq!(
            execution
                .receipt
                .remote_slice_result_id
                .as_ref()
                .map(|id| id.as_str()),
            Some("result-1")
        );
        assert_eq!(
            execution
                .receipt
                .attestation_envelope_id
                .as_ref()
                .map(|id| id.as_str()),
            Some("attestation-1")
        );
        assert_eq!(
            execution
                .receipt
                .cross_runtime_replay_ticket_id
                .as_ref()
                .map(|id| id.as_str()),
            Some("cross-runtime-replay-ticket-1")
        );
        assert_eq!(execution.receipt.planner_stage, ToolPlannerStage::Audit);
        assert_eq!(sink.0.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn runtime_receipt_normalizes_into_canonical_forge_receipt() {
        let mut registry = ToolRegistry::new();
        registry.register(EchoTool {
            descriptor: echo_descriptor(),
        });
        let sink = Arc::new(RecordingSink(std::sync::Mutex::new(Vec::new())));
        let runtime = ToolRuntime::new(registry).with_receipt_sink(sink);
        let call = ToolCall::new(
            "echo",
            "1.0.0",
            json!({"message": "canonical"}),
            ToolOriginKind::Test,
        );
        let mut ctx = tool_ctx();
        ctx.deadline = Some("2026-03-12T00:00:10Z".into());
        ctx.workload_class = Some("verification".into());
        ctx.budget_context = Some(ToolBudgetContext {
            budget_kind: Some("control_plane".into()),
            max_steps: Some(3),
            time_budget_ms: Some(750),
            cost_budget_units: Some(12),
        });
        ctx.parent_receipt_id = Some("dispatch-1".into());
        ctx.family_receipt_id = Some("attempt-family-1".into());
        ctx.replay_parent_receipt_id = Some("retry-0".into());

        let execution = runtime.execute(&ctx, &call, None, None).await;
        let receipt = execution.receipt;
        let forge_receipt = receipt.to_forge_tool_receipt_v2(json!({
            "origin_kind": "test",
            "payload_kind": "echo",
        }));

        assert_eq!(forge_receipt.schema_version, FORGE_TOOL_RECEIPT_V2_SCHEMA);
        assert_eq!(forge_receipt.receipt_id, receipt.receipt_id);
        assert_eq!(forge_receipt.trace_ctx, receipt.trace_ctx);
        assert_eq!(forge_receipt.attempt_id, receipt.attempt_id);
        assert_eq!(forge_receipt.trial_id, receipt.trial_id);
        assert_eq!(forge_receipt.backend_kind, "local_function");
        assert_eq!(forge_receipt.approval_state, "not_required");
        assert_eq!(forge_receipt.planner_stage, "execution");
        assert_eq!(forge_receipt.retry_owner, "llm_pipeline");
        assert_eq!(
            forge_receipt.parent_receipt_id.as_deref(),
            Some("dispatch-1")
        );
        assert_eq!(
            forge_receipt.family_receipt_id.as_deref(),
            Some("attempt-family-1")
        );

        let reparsed: semantic_memory_forge::ForgeToolReceiptV2 =
            serde_json::from_str(&serde_json::to_string(&forge_receipt).unwrap()).unwrap();
        assert_eq!(reparsed.receipt_id, forge_receipt.receipt_id);
        assert_eq!(reparsed.trace_ctx, forge_receipt.trace_ctx);
        assert_eq!(reparsed.raw_payload["payload_kind"], "echo");
    }

    #[tokio::test]
    async fn runtime_requires_execution_permit_for_write_tools() {
        let mut registry = ToolRegistry::new();
        let mut descriptor = echo_descriptor();
        descriptor.name = "submit_patch".into();
        descriptor.read_only = false;
        descriptor.side_effect_class = crate::ToolSideEffectClass::Write;
        descriptor.approval_kind = ToolApprovalKind::UserRequired;
        descriptor.receipt_persistence = ToolReceiptPersistence::Ephemeral;
        descriptor.input_schema = json!({
            "type": "object",
            "required": ["target", "message"],
            "properties": {
                "target": {"type": "string"},
                "message": {"type": "string"}
            },
            "additionalProperties": false
        });
        registry.register(EchoTool { descriptor });
        let runtime = ToolRuntime::new(registry);
        let call = ToolCall::new(
            "submit_patch",
            "1.0.0",
            json!({"target": "src/lib.rs", "message": "hello"}),
            ToolOriginKind::Test,
        );

        let execution = runtime.execute(&tool_ctx(), &call, None, None).await;
        let error = execution.result.unwrap_err();

        assert_eq!(error.class, ToolErrorClass::ApprovalRequired);
    }

    #[tokio::test]
    async fn runtime_requires_typed_grant_for_write_tools_even_with_permit() {
        let mut registry = ToolRegistry::new();
        let mut descriptor = echo_descriptor();
        descriptor.name = "submit_patch".into();
        descriptor.read_only = false;
        descriptor.side_effect_class = crate::ToolSideEffectClass::Write;
        descriptor.approval_kind = ToolApprovalKind::UserRequired;
        descriptor.receipt_persistence = ToolReceiptPersistence::Ephemeral;
        descriptor.input_schema = json!({
            "type": "object",
            "required": ["target", "message"],
            "properties": {
                "target": {"type": "string"},
                "message": {"type": "string"}
            },
            "additionalProperties": false
        });
        registry.register(EchoTool { descriptor });
        let runtime = ToolRuntime::new(registry);
        let call = ToolCall::new(
            "submit_patch",
            "1.0.0",
            json!({"target": "src/lib.rs", "message": "hello"}),
            ToolOriginKind::Test,
        );
        let mut ctx = tool_ctx();
        ctx.scope = Some(stack_ids::ScopeKey::namespace_only("runtime-tests"));
        let permit = execution_permit_for("src/lib.rs");

        let execution = runtime.execute(&ctx, &call, Some(&permit), None).await;
        let error = execution.result.unwrap_err();

        assert_eq!(error.class, ToolErrorClass::ApprovalRequired);
    }

    #[tokio::test]
    async fn runtime_rejects_out_of_scope_execution_permit_for_write_tools() {
        let mut registry = ToolRegistry::new();
        let mut descriptor = echo_descriptor();
        descriptor.name = "submit_patch".into();
        descriptor.read_only = false;
        descriptor.side_effect_class = crate::ToolSideEffectClass::Write;
        descriptor.approval_kind = ToolApprovalKind::UserRequired;
        descriptor.receipt_persistence = ToolReceiptPersistence::Ephemeral;
        descriptor.input_schema = json!({
            "type": "object",
            "required": ["target", "message"],
            "properties": {
                "target": {"type": "string"},
                "message": {"type": "string"}
            },
            "additionalProperties": false
        });
        registry.register(EchoTool { descriptor });
        let runtime = ToolRuntime::new(registry);
        let call = ToolCall::new(
            "submit_patch",
            "1.0.0",
            json!({"target": "src/lib.rs", "message": "hello"}),
            ToolOriginKind::Test,
        );
        let mut ctx = tool_ctx();
        ctx.scope = Some(stack_ids::ScopeKey::namespace_only("runtime-tests"));
        let permit = execution_permit_for("other.rs");

        let execution = runtime.execute(&ctx, &call, Some(&permit), None).await;
        let error = execution.result.unwrap_err();

        assert_eq!(error.class, ToolErrorClass::Denied);
    }

    #[tokio::test]
    async fn runtime_rejects_out_of_scope_grant_for_write_tools() {
        let mut registry = ToolRegistry::new();
        let mut descriptor = echo_descriptor();
        descriptor.name = "submit_patch".into();
        descriptor.read_only = false;
        descriptor.side_effect_class = crate::ToolSideEffectClass::Write;
        descriptor.approval_kind = ToolApprovalKind::UserRequired;
        descriptor.receipt_persistence = ToolReceiptPersistence::Ephemeral;
        descriptor.input_schema = json!({
            "type": "object",
            "required": ["target", "message"],
            "properties": {
                "target": {"type": "string"},
                "message": {"type": "string"}
            },
            "additionalProperties": false
        });
        registry.register(EchoTool { descriptor });
        let runtime = ToolRuntime::new(registry);
        let call = ToolCall::new(
            "submit_patch",
            "1.0.0",
            json!({"target": "src/lib.rs", "message": "hello"}),
            ToolOriginKind::Test,
        );
        let mut ctx = tool_ctx();
        ctx.scope = Some(stack_ids::ScopeKey::namespace_only("runtime-tests"));
        ctx.approval_grant = Some(approval_grant_for(
            "submit_patch",
            crate::ApprovalGrantEffectClass::Write,
            "runtime-tests",
            Some("other.rs"),
        ));
        let permit = execution_permit_for("src/lib.rs");

        let execution = runtime.execute(&ctx, &call, Some(&permit), None).await;
        let error = execution.result.unwrap_err();

        assert_eq!(error.class, ToolErrorClass::Denied);
    }

    #[tokio::test]
    async fn runtime_rejects_expired_grant_for_write_tools() {
        let mut registry = ToolRegistry::new();
        let mut descriptor = echo_descriptor();
        descriptor.name = "submit_patch".into();
        descriptor.read_only = false;
        descriptor.side_effect_class = crate::ToolSideEffectClass::Write;
        descriptor.approval_kind = ToolApprovalKind::UserRequired;
        descriptor.receipt_persistence = ToolReceiptPersistence::Ephemeral;
        descriptor.input_schema = json!({
            "type": "object",
            "required": ["target", "message"],
            "properties": {
                "target": {"type": "string"},
                "message": {"type": "string"}
            },
            "additionalProperties": false
        });
        registry.register(EchoTool { descriptor });
        let runtime = ToolRuntime::new(registry);
        let call = ToolCall::new(
            "submit_patch",
            "1.0.0",
            json!({"target": "src/lib.rs", "message": "hello"}),
            ToolOriginKind::Test,
        );
        let mut ctx = tool_ctx();
        ctx.scope = Some(stack_ids::ScopeKey::namespace_only("runtime-tests"));
        let mut grant = approval_grant_for(
            "submit_patch",
            crate::ApprovalGrantEffectClass::Write,
            "runtime-tests",
            Some("src/lib.rs"),
        );
        grant.expires_at = Some("2000-01-01T00:00:00Z".into());
        ctx.approval_grant = Some(grant);
        let permit = execution_permit_for("src/lib.rs");

        let execution = runtime.execute(&ctx, &call, Some(&permit), None).await;
        let error = execution.result.unwrap_err();

        assert_eq!(error.class, ToolErrorClass::Denied);
    }

    #[tokio::test]
    async fn runtime_rejects_mismatched_effect_grant_for_write_tools() {
        let mut registry = ToolRegistry::new();
        let mut descriptor = echo_descriptor();
        descriptor.name = "submit_patch".into();
        descriptor.read_only = false;
        descriptor.side_effect_class = crate::ToolSideEffectClass::Write;
        descriptor.approval_kind = ToolApprovalKind::UserRequired;
        descriptor.receipt_persistence = ToolReceiptPersistence::Ephemeral;
        descriptor.input_schema = json!({
            "type": "object",
            "required": ["target", "message"],
            "properties": {
                "target": {"type": "string"},
                "message": {"type": "string"}
            },
            "additionalProperties": false
        });
        registry.register(EchoTool { descriptor });
        let runtime = ToolRuntime::new(registry);
        let call = ToolCall::new(
            "submit_patch",
            "1.0.0",
            json!({"target": "src/lib.rs", "message": "hello"}),
            ToolOriginKind::Test,
        );
        let mut ctx = tool_ctx();
        ctx.scope = Some(stack_ids::ScopeKey::namespace_only("runtime-tests"));
        ctx.approval_grant = Some(approval_grant_for(
            "submit_patch",
            crate::ApprovalGrantEffectClass::Analysis,
            "runtime-tests",
            Some("src/lib.rs"),
        ));
        let permit = execution_permit_for("src/lib.rs");

        let execution = runtime.execute(&ctx, &call, Some(&permit), None).await;
        let error = execution.result.unwrap_err();

        assert_eq!(error.class, ToolErrorClass::Denied);
    }

    #[tokio::test]
    async fn runtime_accepts_matching_typed_grant_and_permit_for_write_tools() {
        let mut registry = ToolRegistry::new();
        let mut descriptor = echo_descriptor();
        descriptor.name = "submit_patch".into();
        descriptor.read_only = false;
        descriptor.side_effect_class = crate::ToolSideEffectClass::Write;
        descriptor.approval_kind = ToolApprovalKind::UserRequired;
        descriptor.receipt_persistence = ToolReceiptPersistence::Ephemeral;
        descriptor.input_schema = json!({
            "type": "object",
            "required": ["target", "message"],
            "properties": {
                "target": {"type": "string"},
                "message": {"type": "string"}
            },
            "additionalProperties": false
        });
        registry.register(EchoTool { descriptor });
        let runtime = ToolRuntime::new(registry);
        let call = ToolCall::new(
            "submit_patch",
            "1.0.0",
            json!({"target": "src/lib.rs", "message": "hello"}),
            ToolOriginKind::Test,
        );
        let mut ctx = tool_ctx();
        ctx.scope = Some(stack_ids::ScopeKey::namespace_only("runtime-tests"));
        ctx.approval_grant = Some(approval_grant_for(
            "submit_patch",
            crate::ApprovalGrantEffectClass::Write,
            "runtime-tests",
            Some("src/lib.rs"),
        ));
        let permit = execution_permit_for("src/lib.rs");

        let execution = runtime.execute(&ctx, &call, Some(&permit), None).await;
        assert!(execution.result.is_ok());
        assert_eq!(
            execution.receipt.approval_state,
            ToolApprovalState::Approved
        );
    }
}
