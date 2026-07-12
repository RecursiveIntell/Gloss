use async_trait::async_trait;
use llm_tool_runtime::{
    InMemoryReceiptSink, McpSurfaceKind, Tool, ToolApprovalKind, ToolArtifactRef, ToolBackendKind,
    ToolCall, ToolCtx, ToolDescriptor, ToolError, ToolErrorClass, ToolExposureMode,
    ToolExposurePolicy, ToolIdempotencyClass, ToolJobHandle, ToolOriginKind, ToolOutputMode,
    ToolPlannerStage, ToolReceiptPersistence, ToolRegistry, ToolResult, ToolRetryOwner,
    ToolRuntime, ToolSideEffectClass,
};
use serde_json::json;
use stack_ids::{ArtifactId, AttemptId, TraceCtx, TrialId};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_descriptor(name: &str, backend: ToolBackendKind) -> ToolDescriptor {
    ToolDescriptor {
        name: name.into(),
        version: "1.0.0".into(),
        description: Some(format!("{name} test tool")),
        backend_kind: backend,
        input_schema: json!({
            "type": "object",
            "properties": { "input": { "type": "string" } },
            "additionalProperties": false
        }),
        output_mode: ToolOutputMode::StructuredJson,
        read_only: true,
        side_effect_class: ToolSideEffectClass::ReadOnly,
        idempotency_class: ToolIdempotencyClass::Idempotent,
        approval_kind: ToolApprovalKind::None,
        timeout_ms: 5_000,
        concurrency_key: None,
        cache_ttl_ms: None,
        exposure_mode: ToolExposureMode::Auto,
        mcp_surface_kind: McpSurfaceKind::Tool,
        exposure_policy: ToolExposurePolicy::default(),
        receipt_persistence: ToolReceiptPersistence::Ephemeral,
        output_size_limit_bytes: None,
        provider_payload: None,
    }
}

fn test_ctx() -> ToolCtx {
    ToolCtx {
        trace_ctx: TraceCtx::generate(),
        attempt_id: AttemptId::generate(),
        trial_id: TrialId::generate(),
        caller: "dispatch_test".into(),
        planner_stage: ToolPlannerStage::Execution,
        dry_run: false,
        deadline: None,
        workload_class: None,
        budget_context: None,
        scope: None,
        approval_grant: None,
        execution_permit: None,
        idempotency_key: None,
        parent_receipt_id: None,
        family_receipt_id: None,
        replay_parent_receipt_id: None,
        remote_oracle_lease_id: None,
        remote_slice_result_id: None,
        attestation_envelope_id: None,
        cross_runtime_replay_ticket_id: None,
        retry_owner: None,
    }
}

#[derive(Clone)]
struct TextTool {
    descriptor: ToolDescriptor,
}

#[async_trait]
impl Tool for TextTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }
    async fn invoke(&self, _ctx: &ToolCtx, _call: &ToolCall) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::text("hello from text tool"))
    }
}

#[derive(Clone)]
struct ArtifactTool {
    descriptor: ToolDescriptor,
}

#[async_trait]
impl Tool for ArtifactTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }
    async fn invoke(&self, _ctx: &ToolCtx, _call: &ToolCall) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::artifact_refs(vec![
            ToolArtifactRef {
                artifact_id: ArtifactId::from("artifact-001"),
                label: None,
                mime_type: None,
            },
            ToolArtifactRef {
                artifact_id: ArtifactId::from("artifact-002"),
                label: None,
                mime_type: None,
            },
        ]))
    }
}

#[derive(Clone)]
struct RetryableTool {
    descriptor: ToolDescriptor,
}

#[async_trait]
impl Tool for RetryableTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }
    async fn invoke(&self, _ctx: &ToolCtx, _call: &ToolCall) -> Result<ToolResult, ToolError> {
        let mut err = ToolError::new(ToolErrorClass::Timeout, "timed out, please retry");
        err.retryable = true;
        Err(err)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn error_class_invalid_arguments_properties() {
    let err = ToolError::new(ToolErrorClass::InvalidArguments, "bad args");
    assert_eq!(err.class, ToolErrorClass::InvalidArguments);
    assert!(!err.retryable);
    assert_eq!(err.message, "bad args");
}

#[test]
fn error_class_unknown_tool_properties() {
    let err = ToolError::new(ToolErrorClass::UnknownTool, "not found");
    assert_eq!(err.class, ToolErrorClass::UnknownTool);
    assert!(!err.retryable);
}

#[test]
fn error_class_timeout_properties() {
    let err = ToolError::new(ToolErrorClass::Timeout, "timed out");
    assert_eq!(err.class, ToolErrorClass::Timeout);
}

#[test]
fn error_class_execution_properties() {
    let err = ToolError::new(ToolErrorClass::Execution, "crashed");
    assert_eq!(err.class, ToolErrorClass::Execution);
}

#[test]
fn error_class_denied_properties() {
    let err = ToolError::new(ToolErrorClass::Denied, "nope");
    assert_eq!(err.class, ToolErrorClass::Denied);
}

#[test]
fn error_class_cancelled_properties() {
    let err = ToolError::new(ToolErrorClass::Cancelled, "cancelled by user");
    assert_eq!(err.class, ToolErrorClass::Cancelled);
}

#[test]
fn error_class_output_too_large_properties() {
    let err = ToolError::new(ToolErrorClass::OutputTooLarge, "too big");
    assert_eq!(err.class, ToolErrorClass::OutputTooLarge);
}

#[test]
fn error_class_approval_required_properties() {
    let err = ToolError::new(ToolErrorClass::ApprovalRequired, "need approval");
    assert_eq!(err.class, ToolErrorClass::ApprovalRequired);
}

#[test]
fn error_class_receipt_persistence_properties() {
    let err = ToolError::new(ToolErrorClass::ReceiptPersistence, "failed to persist");
    assert_eq!(err.class, ToolErrorClass::ReceiptPersistence);
}

#[test]
fn error_class_provider_contract_properties() {
    let err = ToolError::new(ToolErrorClass::ProviderContract, "contract violation");
    assert_eq!(err.class, ToolErrorClass::ProviderContract);
}

#[test]
fn error_retryable_flag() {
    let mut err = ToolError::new(ToolErrorClass::Timeout, "timed out");
    assert!(!err.retryable);
    err.retryable = true;
    assert!(err.retryable);
}

#[test]
fn error_with_details() {
    let mut err = ToolError::new(ToolErrorClass::Execution, "bad");
    err.details = Some(json!({"code": 500, "inner": "segfault"}));
    let serialized = serde_json::to_string(&err).unwrap();
    let roundtrip: ToolError = serde_json::from_str(&serialized).unwrap();
    assert_eq!(roundtrip.details.unwrap()["code"], 500);
}

#[test]
fn error_class_serialization_roundtrip() {
    let classes = vec![
        ToolErrorClass::InvalidArguments,
        ToolErrorClass::UnknownTool,
        ToolErrorClass::ApprovalRequired,
        ToolErrorClass::Denied,
        ToolErrorClass::Timeout,
        ToolErrorClass::Cancelled,
        ToolErrorClass::OutputTooLarge,
        ToolErrorClass::Execution,
        ToolErrorClass::ReceiptPersistence,
        ToolErrorClass::ProviderContract,
    ];
    for class in classes {
        let json = serde_json::to_string(&class).unwrap();
        let roundtrip: ToolErrorClass = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip, class);
    }
}

#[test]
fn tool_origin_kind_openai_chat_roundtrips_as_snake_case() {
    let json = serde_json::to_string(&ToolOriginKind::OpenAiChat).unwrap();
    assert_eq!(json, "\"open_ai_chat\"");

    let roundtrip: ToolOriginKind = serde_json::from_str(&json).unwrap();
    assert_eq!(roundtrip, ToolOriginKind::OpenAiChat);

    let call = ToolCall::new("chat_tool", "1.0.0", json!({}), ToolOriginKind::OpenAiChat);
    let call_json = serde_json::to_value(&call).unwrap();
    assert_eq!(call_json["origin_kind"], "open_ai_chat");
}

#[test]
fn retry_owner_as_str_matches_serde() {
    let owners = vec![
        ToolRetryOwner::LlmPipeline,
        ToolRetryOwner::AgentGraph,
        ToolRetryOwner::JobQueue,
        ToolRetryOwner::AiBatchQueue,
        ToolRetryOwner::ForgeOrchestration,
        ToolRetryOwner::External,
    ];
    for owner in owners {
        let json = serde_json::to_string(&owner).unwrap();
        let expected = format!("\"{}\"", owner.as_str());
        assert_eq!(
            json, expected,
            "as_str() and serde must agree for {:?}",
            owner
        );
    }
}

#[test]
fn tool_result_text_mode() {
    let result = ToolResult::text("hello");
    assert_eq!(result.mode, ToolOutputMode::Text);
    let output = result.to_model_output();
    assert!(output.contains("hello"));
}

#[test]
fn tool_result_artifact_refs_mode() {
    let result = ToolResult::artifact_refs(vec![ToolArtifactRef {
        artifact_id: ArtifactId::from("ref-1"),
        label: None,
        mime_type: None,
    }]);
    assert_eq!(result.mode, ToolOutputMode::ArtifactRefs);
}

#[test]
fn tool_result_json_mode() {
    let result = ToolResult::json(json!({"key": "value"}));
    assert_eq!(result.mode, ToolOutputMode::StructuredJson);
}

#[test]
fn tool_result_job_handle_mode() {
    let result = ToolResult::job_handle(ToolJobHandle {
        job_id: "job-123".into(),
        status: None,
    });
    assert_eq!(result.mode, ToolOutputMode::JobHandle);
}

#[test]
fn tool_call_generates_unique_run_id() {
    let call1 = ToolCall::new("test", "1.0", json!({}), ToolOriginKind::Test);
    let call2 = ToolCall::new("test", "1.0", json!({}), ToolOriginKind::Test);
    assert_ne!(call1.tool_run_id, call2.tool_run_id);
}

#[test]
fn registry_multiple_tools() {
    let mut registry = ToolRegistry::new();
    let t1 = TextTool {
        descriptor: test_descriptor("tool_a", ToolBackendKind::LocalFunction),
    };
    let t2 = ArtifactTool {
        descriptor: test_descriptor("tool_b", ToolBackendKind::LocalFunction),
    };
    registry.register(t1);
    registry.register(t2);
    assert!(registry.get("tool_a").is_some());
    assert!(registry.get("tool_b").is_some());
    assert!(registry.get("tool_c").is_none());
    assert_eq!(registry.descriptors().len(), 2);
}

#[tokio::test]
async fn dispatch_routes_to_correct_tool() {
    let mut registry = ToolRegistry::new();
    registry.register(TextTool {
        descriptor: test_descriptor("text_tool", ToolBackendKind::LocalFunction),
    });
    registry.register(ArtifactTool {
        descriptor: test_descriptor("artifact_tool", ToolBackendKind::LocalFunction),
    });
    let runtime = ToolRuntime::new(registry);
    let ctx = test_ctx();

    let text_call = ToolCall::new("text_tool", "1.0.0", json!({}), ToolOriginKind::Test);
    let text_exec = runtime.execute(&ctx, &text_call, None, None).await;
    let text_result = text_exec.result.unwrap();
    assert_eq!(text_result.mode, ToolOutputMode::Text);

    let art_call = ToolCall::new("artifact_tool", "1.0.0", json!({}), ToolOriginKind::Test);
    let art_exec = runtime.execute(&ctx, &art_call, None, None).await;
    let art_result = art_exec.result.unwrap();
    assert_eq!(art_result.mode, ToolOutputMode::ArtifactRefs);
}

#[tokio::test]
async fn dispatch_unknown_tool_returns_error() {
    let registry = ToolRegistry::new();
    let runtime = ToolRuntime::new(registry);
    let ctx = test_ctx();
    let call = ToolCall::new("nonexistent", "1.0.0", json!({}), ToolOriginKind::Test);
    let exec = runtime.execute(&ctx, &call, None, None).await;
    let err = exec.result.unwrap_err();
    assert_eq!(err.class, ToolErrorClass::UnknownTool);
}

#[tokio::test]
async fn receipt_captures_tool_name_and_run_id() {
    let mut registry = ToolRegistry::new();
    registry.register(TextTool {
        descriptor: test_descriptor("my_tool", ToolBackendKind::LocalFunction),
    });
    let runtime = ToolRuntime::new(registry);
    let ctx = test_ctx();
    let call = ToolCall::new("my_tool", "1.0.0", json!({}), ToolOriginKind::Test);
    let exec = runtime.execute(&ctx, &call, None, None).await;
    assert_eq!(exec.receipt.tool_name, "my_tool");
    assert_eq!(exec.receipt.tool_run_id, call.tool_run_id);
}

#[tokio::test]
async fn receipt_error_class_populated_on_failure() {
    let mut registry = ToolRegistry::new();
    registry.register(RetryableTool {
        descriptor: test_descriptor("retry_tool", ToolBackendKind::LocalFunction),
    });
    let runtime = ToolRuntime::new(registry);
    let ctx = test_ctx();
    let call = ToolCall::new("retry_tool", "1.0.0", json!({}), ToolOriginKind::Test);
    let exec = runtime.execute(&ctx, &call, None, None).await;
    assert!(exec.result.is_err());
    assert_eq!(exec.receipt.error_class, Some(ToolErrorClass::Timeout));
}

#[tokio::test]
async fn receipt_sink_collects_receipts() {
    let sink = Arc::new(InMemoryReceiptSink::default());
    let mut registry = ToolRegistry::new();
    let mut desc = test_descriptor("sink_tool", ToolBackendKind::LocalFunction);
    desc.receipt_persistence = ToolReceiptPersistence::ForgeRaw;
    registry.register(TextTool { descriptor: desc });
    let runtime = ToolRuntime::new(registry).with_receipt_sink(sink.clone());
    let ctx = test_ctx();
    let call = ToolCall::new("sink_tool", "1.0.0", json!({}), ToolOriginKind::Test);
    let _ = runtime.execute(&ctx, &call, None, None).await;
    let receipts = sink.receipts();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].tool_name, "sink_tool");
}
