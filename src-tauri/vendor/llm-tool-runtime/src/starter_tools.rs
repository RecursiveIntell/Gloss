use crate::{
    Tool, ToolApprovalKind, ToolBackendKind, ToolCall, ToolCtx, ToolDescriptor, ToolError,
    ToolExposureMode, ToolExposurePolicy, ToolIdempotencyClass, ToolOutputMode,
    ToolReceiptPersistence, ToolResult, ToolSideEffectClass,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use stack_ids::{ArtifactId, ScopeKey};
use std::sync::Arc;

#[async_trait]
pub trait MemorySearchPort: Send + Sync {
    async fn search(
        &self,
        query: &str,
        scope: Option<&ScopeKey>,
        limit: usize,
    ) -> Result<serde_json::Value, ToolError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactContent {
    pub artifact_id: ArtifactId,
    pub content: String,
}

#[async_trait]
pub trait ArtifactReader: Send + Sync {
    async fn read(&self, artifact_id: &ArtifactId) -> Result<ArtifactContent, ToolError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationJobRequest {
    pub candidate_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

#[async_trait]
pub trait RunVerificationPort: Send + Sync {
    async fn enqueue_verification(
        &self,
        request: VerificationJobRequest,
        dry_run: bool,
        execution_permit: &crate::ToolExecutionPermit,
    ) -> Result<crate::ToolJobHandle, ToolError>;
}

#[async_trait]
pub trait ApplyPatchPreviewPort: Send + Sync {
    async fn preview_patch(
        &self,
        target: &str,
        patch: &str,
        execution_permit: &crate::ToolExecutionPermit,
    ) -> Result<serde_json::Value, ToolError>;
}

#[async_trait]
pub trait SubmitPatchPort: Send + Sync {
    async fn submit_patch(
        &self,
        target: &str,
        patch: &str,
        approval_grant: &crate::ApprovalGrant,
        execution_permit: &crate::ToolExecutionPermit,
    ) -> Result<serde_json::Value, ToolError>;
}

fn scoped_execution_permit<'a>(
    ctx: &'a ToolCtx,
    tool_name: &str,
    target_key: &str,
) -> Result<&'a crate::ToolExecutionPermit, ToolError> {
    let permit = ctx.execution_permit.as_ref().ok_or_else(|| {
        ToolError::new(
            crate::ToolErrorClass::ApprovalRequired,
            format!("{tool_name} requires an execution permit"),
        )
    })?;
    let scope = ctx.scope.as_ref().ok_or_else(|| {
        ToolError::new(
            crate::ToolErrorClass::Denied,
            format!("{tool_name} requires a scoped context"),
        )
    })?;
    if permit.scope().namespace() != scope.namespace || permit.scope().target_key() != target_key {
        return Err(ToolError::new(
            crate::ToolErrorClass::Denied,
            format!("{tool_name} execution permit does not cover target {target_key}"),
        ));
    }
    Ok(permit)
}

pub struct SearchMemoryTool<P> {
    port: Arc<P>,
    descriptor: ToolDescriptor,
}

impl<P> SearchMemoryTool<P> {
    /// Builds the default search-memory tool descriptor around the supplied port.
    pub fn new(port: Arc<P>) -> Self {
        Self {
            port,
            descriptor: ToolDescriptor {
                name: "search_memory".into(),
                version: "1.0.0".into(),
                description: Some("Search projected memory within the active scope".into()),
                backend_kind: ToolBackendKind::LocalFunction,
                input_schema: json!({
                    "type": "object",
                    "required": ["query"],
                    "properties": {
                        "query": {"type": "string"},
                        "limit": {"type": "integer"}
                    },
                    "additionalProperties": false
                }),
                output_mode: ToolOutputMode::StructuredJson,
                read_only: true,
                side_effect_class: ToolSideEffectClass::ReadOnly,
                idempotency_class: ToolIdempotencyClass::Idempotent,
                approval_kind: ToolApprovalKind::None,
                timeout_ms: 10_000,
                concurrency_key: Some("search_memory".into()),
                cache_ttl_ms: Some(5_000),
                exposure_mode: ToolExposureMode::Auto,
                mcp_surface_kind: crate::McpSurfaceKind::Tool,
                exposure_policy: ToolExposurePolicy::default(),
                receipt_persistence: ToolReceiptPersistence::Ephemeral,
                output_size_limit_bytes: Some(128 * 1024),
                provider_payload: None,
            },
        }
    }
}

#[async_trait]
impl<P> Tool for SearchMemoryTool<P>
where
    P: MemorySearchPort + 'static,
{
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, ctx: &ToolCtx, call: &ToolCall) -> Result<ToolResult, ToolError> {
        let query = call
            .arguments
            .get("query")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let limit = call
            .arguments
            .get("limit")
            .and_then(|value| value.as_u64())
            .unwrap_or(5) as usize;
        let results = self.port.search(query, ctx.scope.as_ref(), limit).await?;
        Ok(ToolResult::json(results))
    }
}

pub struct ReadArtifactTool<P> {
    port: Arc<P>,
    descriptor: ToolDescriptor,
}

impl<P> ReadArtifactTool<P> {
    /// Builds the default read-artifact tool descriptor around the supplied port.
    pub fn new(port: Arc<P>) -> Self {
        Self {
            port,
            descriptor: ToolDescriptor {
                name: "read_artifact".into(),
                version: "1.0.0".into(),
                description: Some("Read the content of a stored artifact".into()),
                backend_kind: ToolBackendKind::LocalFunction,
                input_schema: json!({
                    "type": "object",
                    "required": ["artifact_id"],
                    "properties": {
                        "artifact_id": {"type": "string"}
                    },
                    "additionalProperties": false
                }),
                output_mode: ToolOutputMode::Text,
                read_only: true,
                side_effect_class: ToolSideEffectClass::ReadOnly,
                idempotency_class: ToolIdempotencyClass::Idempotent,
                approval_kind: ToolApprovalKind::None,
                timeout_ms: 10_000,
                concurrency_key: Some("read_artifact".into()),
                cache_ttl_ms: Some(5_000),
                exposure_mode: ToolExposureMode::OptIn,
                mcp_surface_kind: crate::McpSurfaceKind::Resource,
                exposure_policy: ToolExposurePolicy::default(),
                receipt_persistence: ToolReceiptPersistence::Ephemeral,
                output_size_limit_bytes: Some(256 * 1024),
                provider_payload: None,
            },
        }
    }
}

#[async_trait]
impl<P> Tool for ReadArtifactTool<P>
where
    P: ArtifactReader + 'static,
{
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, _ctx: &ToolCtx, call: &ToolCall) -> Result<ToolResult, ToolError> {
        let artifact_id = call
            .arguments
            .get("artifact_id")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let content = self.port.read(&ArtifactId::new(artifact_id)).await?;
        Ok(ToolResult::text(content.content))
    }
}

pub struct RunVerificationTool<P> {
    port: Arc<P>,
    descriptor: ToolDescriptor,
}

impl<P> RunVerificationTool<P> {
    /// Builds the default run-verification tool descriptor around the supplied port.
    pub fn new(port: Arc<P>) -> Self {
        Self {
            port,
            descriptor: ToolDescriptor {
                name: "run_verification".into(),
                version: "1.0.0".into(),
                description: Some(
                    "Enqueue a verification job and return a durable job handle".into(),
                ),
                backend_kind: ToolBackendKind::LocalFunction,
                input_schema: json!({
                    "type": "object",
                    "required": ["candidate_id"],
                    "properties": {
                        "candidate_id": {"type": "string"},
                        "task_id": {"type": "string"}
                    },
                    "additionalProperties": false
                }),
                output_mode: ToolOutputMode::JobHandle,
                read_only: false,
                side_effect_class: ToolSideEffectClass::Analysis,
                idempotency_class: ToolIdempotencyClass::BestEffort,
                approval_kind: ToolApprovalKind::PolicyRequired,
                timeout_ms: 30_000,
                concurrency_key: Some("run_verification".into()),
                cache_ttl_ms: None,
                exposure_mode: ToolExposureMode::OptIn,
                mcp_surface_kind: crate::McpSurfaceKind::Tool,
                exposure_policy: ToolExposurePolicy::default(),
                receipt_persistence: ToolReceiptPersistence::ForgeRaw,
                output_size_limit_bytes: Some(8 * 1024),
                provider_payload: None,
            },
        }
    }
}

#[async_trait]
impl<P> Tool for RunVerificationTool<P>
where
    P: RunVerificationPort + 'static,
{
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, ctx: &ToolCtx, call: &ToolCall) -> Result<ToolResult, ToolError> {
        let request: VerificationJobRequest = serde_json::from_value(call.arguments.clone())
            .map_err(|error| ToolError {
                class: crate::ToolErrorClass::InvalidArguments,
                message: error.to_string(),
                retryable: false,
                details: None,
            })?;
        let permit =
            scoped_execution_permit(ctx, self.descriptor.name.as_str(), &request.candidate_id)?;
        let handle = self
            .port
            .enqueue_verification(request, ctx.dry_run, permit)
            .await?;
        Ok(ToolResult::job_handle(handle))
    }
}

pub struct ApplyPatchPreviewTool<P> {
    port: Arc<P>,
    descriptor: ToolDescriptor,
}

impl<P> ApplyPatchPreviewTool<P> {
    /// Builds the default patch-preview tool descriptor around the supplied port.
    pub fn new(port: Arc<P>) -> Self {
        Self {
            port,
            descriptor: ToolDescriptor {
                name: "apply_patch_preview".into(),
                version: "1.0.0".into(),
                description: Some("Preview a patch application without mutating the target".into()),
                backend_kind: ToolBackendKind::LocalFunction,
                input_schema: json!({
                    "type": "object",
                    "required": ["target", "patch"],
                    "properties": {
                        "target": {"type": "string"},
                        "patch": {"type": "string"}
                    },
                    "additionalProperties": false
                }),
                output_mode: ToolOutputMode::StructuredJson,
                read_only: false,
                side_effect_class: ToolSideEffectClass::PreviewWrite,
                idempotency_class: ToolIdempotencyClass::Idempotent,
                approval_kind: ToolApprovalKind::PolicyRequired,
                timeout_ms: 10_000,
                concurrency_key: Some("apply_patch_preview".into()),
                cache_ttl_ms: None,
                exposure_mode: ToolExposureMode::OptIn,
                mcp_surface_kind: crate::McpSurfaceKind::Tool,
                exposure_policy: ToolExposurePolicy::default(),
                receipt_persistence: ToolReceiptPersistence::ForgeRaw,
                output_size_limit_bytes: Some(64 * 1024),
                provider_payload: None,
            },
        }
    }
}

#[async_trait]
impl<P> Tool for ApplyPatchPreviewTool<P>
where
    P: ApplyPatchPreviewPort + 'static,
{
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, _ctx: &ToolCtx, call: &ToolCall) -> Result<ToolResult, ToolError> {
        let target = call
            .arguments
            .get("target")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let patch = call
            .arguments
            .get("patch")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let permit = scoped_execution_permit(_ctx, self.descriptor.name.as_str(), target)?;
        let preview = self.port.preview_patch(target, patch, permit).await?;
        Ok(ToolResult::json(preview))
    }
}

pub struct SubmitPatchTool<P> {
    port: Arc<P>,
    descriptor: ToolDescriptor,
}

impl<P> SubmitPatchTool<P> {
    /// Builds the default patch-submission tool descriptor around the supplied port.
    pub fn new(port: Arc<P>) -> Self {
        Self {
            port,
            descriptor: ToolDescriptor {
                name: "submit_patch".into(),
                version: "1.0.0".into(),
                description: Some("Submit a patch to the target with explicit approval".into()),
                backend_kind: ToolBackendKind::LocalFunction,
                input_schema: json!({
                    "type": "object",
                    "required": ["target", "patch"],
                    "properties": {
                        "target": {"type": "string"},
                        "patch": {"type": "string"}
                    },
                    "additionalProperties": false
                }),
                output_mode: ToolOutputMode::StructuredJson,
                read_only: false,
                side_effect_class: ToolSideEffectClass::Write,
                idempotency_class: ToolIdempotencyClass::NonIdempotent,
                approval_kind: ToolApprovalKind::UserRequired,
                timeout_ms: 10_000,
                concurrency_key: Some("submit_patch".into()),
                cache_ttl_ms: None,
                exposure_mode: ToolExposureMode::Hidden,
                mcp_surface_kind: crate::McpSurfaceKind::Tool,
                exposure_policy: ToolExposurePolicy {
                    exposure_mode: ToolExposureMode::Hidden,
                    ..Default::default()
                },
                receipt_persistence: ToolReceiptPersistence::ForgeRaw,
                output_size_limit_bytes: Some(64 * 1024),
                provider_payload: None,
            },
        }
    }
}

#[async_trait]
impl<P> Tool for SubmitPatchTool<P>
where
    P: SubmitPatchPort + 'static,
{
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, ctx: &ToolCtx, call: &ToolCall) -> Result<ToolResult, ToolError> {
        let target = call
            .arguments
            .get("target")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let patch = call
            .arguments
            .get("patch")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let permit = scoped_execution_permit(ctx, self.descriptor.name.as_str(), target)?;
        let result = self
            .port
            .submit_patch(
                target,
                patch,
                ctx.approval_grant.as_ref().ok_or_else(|| {
                    ToolError::new(
                        crate::ToolErrorClass::ApprovalRequired,
                        "submit_patch requires an approval grant",
                    )
                })?,
                permit,
            )
            .await?;
        Ok(ToolResult::json(result))
    }
}

pub struct StarterToolPorts<MS, AR, RV, PP, SP> {
    pub memory_search: Arc<MS>,
    pub artifact_reader: Arc<AR>,
    pub verification: Arc<RV>,
    pub patch_preview: Arc<PP>,
    pub patch_submit: Arc<SP>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ToolCall, ToolCtx, ToolOriginKind, ToolRetryOwner};
    use serde_json::json;
    use stack_ids::{ArtifactId, AttemptId, TraceCtx, TrialId};

    struct TestMemorySearch;

    #[async_trait]
    impl MemorySearchPort for TestMemorySearch {
        async fn search(
            &self,
            query: &str,
            scope: Option<&ScopeKey>,
            limit: usize,
        ) -> Result<serde_json::Value, ToolError> {
            Ok(json!({
                "query": query,
                "scope": scope.map(|scope| scope.namespace.clone()),
                "limit": limit,
            }))
        }
    }

    struct TestArtifactReader;

    #[async_trait]
    impl ArtifactReader for TestArtifactReader {
        async fn read(&self, artifact_id: &ArtifactId) -> Result<ArtifactContent, ToolError> {
            Ok(ArtifactContent {
                artifact_id: artifact_id.clone(),
                content: format!("artifact:{}", artifact_id.as_str()),
            })
        }
    }

    struct TestVerificationPort;

    #[async_trait]
    impl RunVerificationPort for TestVerificationPort {
        async fn enqueue_verification(
            &self,
            request: VerificationJobRequest,
            dry_run: bool,
            execution_permit: &crate::ToolExecutionPermit,
        ) -> Result<crate::ToolJobHandle, ToolError> {
            Ok(crate::ToolJobHandle {
                job_id: format!(
                    "job:{}:{}",
                    request.candidate_id,
                    execution_permit.execution_permit_id()
                ),
                status: Some(if dry_run { "dry_run" } else { "queued" }.into()),
            })
        }
    }

    struct TestPatchPreviewPort;

    #[async_trait]
    impl ApplyPatchPreviewPort for TestPatchPreviewPort {
        async fn preview_patch(
            &self,
            target: &str,
            patch: &str,
            execution_permit: &crate::ToolExecutionPermit,
        ) -> Result<serde_json::Value, ToolError> {
            Ok(json!({
                "target": target,
                "patch_len": patch.len(),
                "execution_permit_id": execution_permit.execution_permit_id(),
                "preview": true,
            }))
        }
    }

    struct TestSubmitPatchPort;

    #[async_trait]
    impl SubmitPatchPort for TestSubmitPatchPort {
        async fn submit_patch(
            &self,
            target: &str,
            patch: &str,
            approval_grant: &crate::ApprovalGrant,
            execution_permit: &crate::ToolExecutionPermit,
        ) -> Result<serde_json::Value, ToolError> {
            Ok(json!({
                "target": target,
                "patch_len": patch.len(),
                "approval_grant_id": approval_grant.grant_id,
                "execution_permit_id": execution_permit.execution_permit_id(),
                "submitted": true,
            }))
        }
    }

    fn execution_permit_for(target_key: &str) -> crate::ToolExecutionPermit {
        crate::ToolExecutionPermit::new(
            stack_ids::ExecutionPermitId::generate(),
            stack_ids::PolicyDecisionId::generate(),
            None,
            "starter-tools-test",
            target_key,
        )
    }

    fn tool_ctx() -> ToolCtx {
        ToolCtx {
            trace_ctx: TraceCtx::generate(),
            attempt_id: AttemptId::generate(),
            trial_id: TrialId::generate(),
            deadline: None,
            workload_class: None,
            budget_context: None,
            scope: Some(ScopeKey::namespace_only("starter-tools-test")),
            dry_run: true,
            approval_grant: Some(crate::ApprovalGrant::new(
                "policy-1",
                crate::ApprovalGrantScope {
                    namespace: "starter-tools-test".into(),
                    target_key: Some("src/lib.rs".into()),
                    tool_name: "submit_patch".into(),
                    effect_class: crate::ApprovalGrantEffectClass::Write,
                    planner_stage: Some(crate::ToolPlannerStage::Execution),
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
            )),
            execution_permit: Some(execution_permit_for("src/lib.rs")),
            idempotency_key: None,
            caller: "starter-tools-test".into(),
            planner_stage: crate::ToolPlannerStage::Execution,
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

    #[tokio::test]
    async fn starter_tools_return_expected_results() {
        let ctx = tool_ctx();

        let search_tool = SearchMemoryTool::new(Arc::new(TestMemorySearch));
        let search_result = search_tool
            .invoke(
                &ctx,
                &ToolCall::new(
                    "search_memory",
                    "1.0.0",
                    json!({"query": "tool runtime", "limit": 3}),
                    ToolOriginKind::Test,
                ),
            )
            .await
            .unwrap();
        assert_eq!(search_result.payload["query"], "tool runtime");
        assert_eq!(search_result.payload["limit"], 3);

        let read_tool = ReadArtifactTool::new(Arc::new(TestArtifactReader));
        let read_result = read_tool
            .invoke(
                &ctx,
                &ToolCall::new(
                    "read_artifact",
                    "1.0.0",
                    json!({"artifact_id": "artifact-1"}),
                    ToolOriginKind::Test,
                ),
            )
            .await
            .unwrap();
        assert_eq!(
            read_result.display_text.as_deref(),
            Some("artifact:artifact-1")
        );

        let verification_tool = RunVerificationTool::new(Arc::new(TestVerificationPort));
        let mut verification_ctx = tool_ctx();
        verification_ctx.execution_permit = Some(execution_permit_for("cand-1"));
        let verification_result = verification_tool
            .invoke(
                &verification_ctx,
                &ToolCall::new(
                    "run_verification",
                    "1.0.0",
                    json!({"candidate_id": "cand-1"}),
                    ToolOriginKind::Test,
                ),
            )
            .await
            .unwrap();
        assert!(verification_result.payload["job_id"]
            .as_str()
            .is_some_and(|job_id| job_id.starts_with("job:cand-1:")));

        let preview_tool = ApplyPatchPreviewTool::new(Arc::new(TestPatchPreviewPort));
        let preview_result = preview_tool
            .invoke(
                &ctx,
                &ToolCall::new(
                    "apply_patch_preview",
                    "1.0.0",
                    json!({"target": "src/lib.rs", "patch": "*** Begin Patch"}),
                    ToolOriginKind::Test,
                ),
            )
            .await
            .unwrap();
        assert_eq!(preview_result.payload["preview"], true);
        assert!(preview_result.payload["execution_permit_id"].is_string());

        let submit_tool = SubmitPatchTool::new(Arc::new(TestSubmitPatchPort));
        let submit_result = submit_tool
            .invoke(
                &ctx,
                &ToolCall::new(
                    "submit_patch",
                    "1.0.0",
                    json!({"target": "src/lib.rs", "patch": "*** Begin Patch"}),
                    ToolOriginKind::Test,
                ),
            )
            .await
            .unwrap();
        assert_eq!(submit_result.payload["submitted"], true);
        assert!(submit_result.payload["approval_grant_id"].is_string());
        assert!(submit_result.payload["execution_permit_id"].is_string());
    }
}
