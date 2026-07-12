use async_trait::async_trait;
use llm_tool_runtime::{
    validate_arguments_against_schema, InMemoryReceiptSink, McpSurfaceKind, Tool, ToolApprovalKind,
    ToolApprovalState, ToolBackendKind, ToolBudgetContext, ToolCall, ToolCtx, ToolDescriptor,
    ToolError, ToolErrorClass, ToolExposureMode, ToolExposurePolicy, ToolExposureRequest,
    ToolIdempotencyClass, ToolOriginKind, ToolOutputMode, ToolPlannerStage, ToolReceipt,
    ToolReceiptPersistence, ToolReceiptSink, ToolRegistry, ToolResult, ToolRetryOwner, ToolRuntime,
    ToolSideEffectClass,
};
use serde_json::json;
use stack_ids::{AttemptId, ContentDigest, RemoteOracleLeaseId, ScopeKey, TraceCtx, TrialId};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct EchoTool {
    descriptor: ToolDescriptor,
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

struct FailingTool {
    descriptor: ToolDescriptor,
}

#[async_trait]
impl Tool for FailingTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    async fn invoke(&self, _ctx: &ToolCtx, _call: &ToolCall) -> Result<ToolResult, ToolError> {
        Err(ToolError::new(
            ToolErrorClass::Execution,
            "intentional test failure",
        ))
    }
}

fn read_only_descriptor(name: &str) -> ToolDescriptor {
    ToolDescriptor {
        name: name.into(),
        version: "1.0.0".into(),
        description: Some(format!("{name} tool")),
        backend_kind: ToolBackendKind::LocalFunction,
        input_schema: json!({
            "type": "object",
            "required": ["query"],
            "properties": { "query": { "type": "string" } },
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
        caller: "core-tests".into(),
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

fn make_receipt(
    tool_name: &str,
    retry_owner: ToolRetryOwner,
    error_class: Option<ToolErrorClass>,
) -> ToolReceipt {
    let ctx = tool_ctx();
    ToolReceipt {
        receipt_id: uuid::Uuid::new_v4().to_string(),
        tool_name: tool_name.into(),
        tool_version: "1.0.0".into(),
        backend_kind: ToolBackendKind::LocalFunction,
        input_digest: ContentDigest::compute(b"test-input"),
        output_digest_or_refs: json!({"digest": "abc123"}),
        policy_hash: ContentDigest::compute(b"test-policy"),
        approval_state: ToolApprovalState::NotRequired,
        host_identity: "core-tests".into(),
        started_at: "2026-03-25T00:00:00Z".into(),
        finished_at: "2026-03-25T00:00:01Z".into(),
        trace_ctx: ctx.trace_ctx,
        attempt_id: ctx.attempt_id,
        trial_id: ctx.trial_id,
        planner_stage: ToolPlannerStage::Execution,
        deadline: None,
        workload_class: None,
        budget_context: None,
        parent_receipt_id: None,
        family_receipt_id: None,
        replay_parent_receipt_id: None,
        remote_oracle_lease_id: None,
        remote_slice_result_id: None,
        attestation_envelope_id: None,
        cross_runtime_replay_ticket_id: None,
        error_class,
        retry_owner,
        replay_link: Some("tool_run:abc".into()),
        tool_run_id: "run-1".into(),
        provider_call_id: None,
    }
}

// ===========================================================================
// 1. ToolReceipt construction with all fields
// ===========================================================================

#[test]
fn test_tool_receipt_construction() {
    let receipt = make_receipt("echo", ToolRetryOwner::LlmPipeline, None);

    assert_eq!(receipt.tool_name, "echo");
    assert_eq!(receipt.tool_version, "1.0.0");
    assert_eq!(receipt.backend_kind, ToolBackendKind::LocalFunction);
    assert_eq!(receipt.approval_state, ToolApprovalState::NotRequired);
    assert_eq!(receipt.host_identity, "core-tests");
    assert_eq!(receipt.started_at, "2026-03-25T00:00:00Z");
    assert_eq!(receipt.finished_at, "2026-03-25T00:00:01Z");
    assert_eq!(receipt.planner_stage, ToolPlannerStage::Execution);
    assert!(receipt.error_class.is_none());
    assert_eq!(receipt.retry_owner, ToolRetryOwner::LlmPipeline);
    assert_eq!(receipt.tool_run_id, "run-1");
    assert!(receipt.replay_link.as_deref() == Some("tool_run:abc"));
}

// ===========================================================================
// 2. ToolReceipt serialization roundtrip
// ===========================================================================

#[test]
fn test_tool_receipt_serialization_roundtrip() {
    let receipt = make_receipt("echo", ToolRetryOwner::AgentGraph, None);

    let json_str = serde_json::to_string(&receipt).unwrap();
    let deserialized: ToolReceipt = serde_json::from_str(&json_str).unwrap();

    assert_eq!(deserialized.receipt_id, receipt.receipt_id);
    assert_eq!(deserialized.tool_name, receipt.tool_name);
    assert_eq!(deserialized.tool_version, receipt.tool_version);
    assert_eq!(deserialized.backend_kind, receipt.backend_kind);
    assert_eq!(deserialized.approval_state, receipt.approval_state);
    assert_eq!(deserialized.retry_owner, receipt.retry_owner);
    assert_eq!(deserialized.planner_stage, receipt.planner_stage);
    assert_eq!(deserialized.trace_ctx, receipt.trace_ctx);
    assert_eq!(deserialized.attempt_id, receipt.attempt_id);
    assert_eq!(deserialized.trial_id, receipt.trial_id);
    assert_eq!(deserialized.tool_run_id, receipt.tool_run_id);
}

// ===========================================================================
// 3. ToolReceipt serialization roundtrip with all optional fields
// ===========================================================================

#[test]
fn test_tool_receipt_serialization_roundtrip_all_optional_fields() {
    let mut receipt = make_receipt(
        "echo",
        ToolRetryOwner::JobQueue,
        Some(ToolErrorClass::Timeout),
    );
    receipt.deadline = Some("2026-03-25T01:00:00Z".into());
    receipt.workload_class = Some("verification".into());
    receipt.budget_context = Some(ToolBudgetContext {
        budget_kind: Some("control_plane".into()),
        max_steps: Some(5),
        time_budget_ms: Some(1000),
        cost_budget_units: Some(42),
    });
    receipt.parent_receipt_id = Some("parent-1".into());
    receipt.family_receipt_id = Some("family-1".into());
    receipt.replay_parent_receipt_id = Some("replay-parent-1".into());
    receipt.remote_oracle_lease_id = Some(RemoteOracleLeaseId::new("lease-1"));
    receipt.provider_call_id = Some("provider-call-1".into());

    let json_str = serde_json::to_string(&receipt).unwrap();
    let deserialized: ToolReceipt = serde_json::from_str(&json_str).unwrap();

    assert_eq!(
        deserialized.deadline.as_deref(),
        Some("2026-03-25T01:00:00Z")
    );
    assert_eq!(deserialized.workload_class.as_deref(), Some("verification"));
    assert_eq!(
        deserialized
            .budget_context
            .as_ref()
            .and_then(|b| b.max_steps),
        Some(5)
    );
    assert_eq!(deserialized.parent_receipt_id.as_deref(), Some("parent-1"));
    assert_eq!(deserialized.family_receipt_id.as_deref(), Some("family-1"));
    assert_eq!(
        deserialized.replay_parent_receipt_id.as_deref(),
        Some("replay-parent-1")
    );
    assert_eq!(
        deserialized
            .remote_oracle_lease_id
            .as_ref()
            .map(|id| id.as_str()),
        Some("lease-1")
    );
    assert_eq!(deserialized.error_class, Some(ToolErrorClass::Timeout));
    assert_eq!(
        deserialized.provider_call_id.as_deref(),
        Some("provider-call-1")
    );
}

// ===========================================================================
// 4. ToolReceipt with retry owner set — field present in serialized output
// ===========================================================================

#[test]
fn test_tool_receipt_with_retry_owner_serialized() {
    for (owner, expected_str) in [
        (ToolRetryOwner::LlmPipeline, "llm_pipeline"),
        (ToolRetryOwner::AgentGraph, "agent_graph"),
        (ToolRetryOwner::JobQueue, "job_queue"),
        (ToolRetryOwner::AiBatchQueue, "ai_batch_queue"),
        (ToolRetryOwner::ForgeOrchestration, "forge_orchestration"),
        (ToolRetryOwner::External, "external"),
    ] {
        let receipt = make_receipt("echo", owner.clone(), None);
        let json_val: serde_json::Value = serde_json::to_value(&receipt).unwrap();

        assert_eq!(
            json_val["retry_owner"].as_str().unwrap(),
            expected_str,
            "ToolRetryOwner::{:?} should serialize to {expected_str}",
            owner
        );
    }
}

// ===========================================================================
// 5. ToolRetryOwner lifecycle: each variant has a stable as_str label
// ===========================================================================

#[test]
fn test_tool_retry_owner_lifecycle() {
    let variants = [
        (ToolRetryOwner::LlmPipeline, "llm_pipeline"),
        (ToolRetryOwner::AgentGraph, "agent_graph"),
        (ToolRetryOwner::JobQueue, "job_queue"),
        (ToolRetryOwner::AiBatchQueue, "ai_batch_queue"),
        (ToolRetryOwner::ForgeOrchestration, "forge_orchestration"),
        (ToolRetryOwner::External, "external"),
    ];

    for (variant, expected_label) in variants {
        assert_eq!(variant.as_str(), expected_label);

        // Roundtrip through serde confirms identity is preserved
        let json_str = serde_json::to_string(&variant).unwrap();
        let roundtripped: ToolRetryOwner = serde_json::from_str(&json_str).unwrap();
        assert_eq!(roundtripped.as_str(), expected_label);
    }
}

// ===========================================================================
// 6. Registry: register tool, lookup by name
// ===========================================================================

#[test]
fn test_registry_register_and_lookup() {
    let mut registry = ToolRegistry::new();
    registry.register(EchoTool {
        descriptor: read_only_descriptor("alpha"),
    });

    let tool = registry.get("alpha").unwrap();
    assert_eq!(tool.descriptor().name, "alpha");
    assert_eq!(tool.descriptor().version, "1.0.0");

    let descriptors = registry.descriptors();
    assert_eq!(descriptors.len(), 1);
    assert_eq!(descriptors[0].name, "alpha");
}

// ===========================================================================
// 7. Registry: lookup missing returns None
// ===========================================================================

#[test]
fn test_registry_lookup_missing() {
    let registry = ToolRegistry::new();
    assert!(registry.get("nonexistent").is_none());

    let mut registry_with_one = ToolRegistry::new();
    registry_with_one.register(EchoTool {
        descriptor: read_only_descriptor("alpha"),
    });
    assert!(registry_with_one.get("beta").is_none());
}

// ===========================================================================
// 8. Registry: duplicate registration replaces the previous entry
// ===========================================================================

#[test]
fn test_registry_duplicate_handling() {
    let mut registry = ToolRegistry::new();

    let mut desc_v1 = read_only_descriptor("dup_tool");
    desc_v1.version = "1.0.0".into();
    desc_v1.description = Some("version one".into());
    registry.register(EchoTool {
        descriptor: desc_v1,
    });

    let mut desc_v2 = read_only_descriptor("dup_tool");
    desc_v2.version = "2.0.0".into();
    desc_v2.description = Some("version two".into());
    registry.register(EchoTool {
        descriptor: desc_v2,
    });

    // The second registration replaces the first
    let tool = registry.get("dup_tool").unwrap();
    assert_eq!(tool.descriptor().version, "2.0.0");
    assert_eq!(
        tool.descriptor().description.as_deref(),
        Some("version two")
    );

    // Total count is still 1
    assert_eq!(registry.descriptors().len(), 1);
}

// ===========================================================================
// 9. Provider: render_openai_tool dispatches to correct provider
// ===========================================================================

#[test]
fn test_provider_dispatch_correct() {
    let descriptor = read_only_descriptor("local_echo");

    let rendered = llm_tool_runtime::render_openai_tool(&descriptor, true).unwrap();
    assert_eq!(rendered["type"], "function");
    assert_eq!(rendered["name"], "local_echo");
    assert_eq!(rendered["strict"], true);
    assert_eq!(rendered["parameters"]["type"], "object");

    // Ollama rendering also works for LocalFunction
    let ollama_rendered = llm_tool_runtime::render_ollama_tool(&descriptor).unwrap();
    assert_eq!(ollama_rendered["type"], "function");
    assert_eq!(ollama_rendered["function"]["name"], "local_echo");
}

// ===========================================================================
// 10. Provider: error propagation on incompatible backend
// ===========================================================================

#[test]
fn test_provider_dispatch_error() {
    let mut descriptor = read_only_descriptor("ollama_only");
    descriptor.backend_kind = ToolBackendKind::OllamaFunction;

    // OllamaFunction cannot be rendered for OpenAI
    let err = llm_tool_runtime::render_openai_tool(&descriptor, false).unwrap_err();
    assert_eq!(err.class, ToolErrorClass::ProviderContract);
    assert!(
        err.message.contains("Ollama-only"),
        "Error message should indicate Ollama-only tool, got: {}",
        err.message
    );

    // RemoteMcp without provider_payload fails
    let mut mcp_descriptor = read_only_descriptor("mcp_tool");
    mcp_descriptor.backend_kind = ToolBackendKind::RemoteMcp;
    let err = llm_tool_runtime::render_openai_tool(&mcp_descriptor, false).unwrap_err();
    assert_eq!(err.class, ToolErrorClass::ProviderContract);
    assert!(err.message.contains("provider_payload"));
}

// ===========================================================================
// 11. StarterTools: search_memory tool produces valid result
// ===========================================================================

#[tokio::test]
async fn test_starter_tool_valid_receipt() {
    use llm_tool_runtime::SearchMemoryTool;

    struct TestSearch;

    #[async_trait]
    impl llm_tool_runtime::MemorySearchPort for TestSearch {
        async fn search(
            &self,
            query: &str,
            _scope: Option<&ScopeKey>,
            limit: usize,
        ) -> Result<serde_json::Value, ToolError> {
            Ok(json!({"query": query, "limit": limit, "results": []}))
        }
    }

    let tool = SearchMemoryTool::new(Arc::new(TestSearch));
    assert_eq!(tool.descriptor().name, "search_memory");
    assert_eq!(tool.descriptor().version, "1.0.0");
    assert!(tool.descriptor().read_only);

    let ctx = tool_ctx();
    let call = ToolCall::new(
        "search_memory",
        "1.0.0",
        json!({"query": "governance artifacts", "limit": 10}),
        ToolOriginKind::Test,
    );

    let result = tool.invoke(&ctx, &call).await.unwrap();
    assert_eq!(result.payload["query"], "governance artifacts");
    assert_eq!(result.payload["limit"], 10);
    assert_eq!(result.mode, ToolOutputMode::StructuredJson);
}

// ===========================================================================
// 12. StarterTools: read_artifact with invalid (empty) artifact_id
// ===========================================================================

#[tokio::test]
async fn test_starter_tool_invalid_input() {
    use llm_tool_runtime::ReadArtifactTool;

    struct FailingReader;

    #[async_trait]
    impl llm_tool_runtime::ArtifactReader for FailingReader {
        async fn read(
            &self,
            artifact_id: &stack_ids::ArtifactId,
        ) -> Result<llm_tool_runtime::ArtifactContent, ToolError> {
            Err(ToolError::new(
                ToolErrorClass::InvalidArguments,
                format!("artifact not found: {}", artifact_id.as_str()),
            ))
        }
    }

    let tool = ReadArtifactTool::new(Arc::new(FailingReader));
    let ctx = tool_ctx();
    let call = ToolCall::new(
        "read_artifact",
        "1.0.0",
        json!({"artifact_id": ""}),
        ToolOriginKind::Test,
    );

    let err = tool.invoke(&ctx, &call).await.unwrap_err();
    assert_eq!(err.class, ToolErrorClass::InvalidArguments);
    assert!(err.message.contains("artifact not found"));
}

// ===========================================================================
// 13. Runtime: execute tool with valid contract (read-only, ephemeral)
// ===========================================================================

#[tokio::test]
async fn test_runtime_execute_valid() {
    let mut registry = ToolRegistry::new();
    registry.register(EchoTool {
        descriptor: read_only_descriptor("ping"),
    });
    let runtime = ToolRuntime::new(registry);
    let call = ToolCall::new(
        "ping",
        "1.0.0",
        json!({"query": "hello"}),
        ToolOriginKind::Test,
    );

    let execution = runtime.execute(&tool_ctx(), &call, None, None).await;
    let result = execution.result.unwrap();
    assert_eq!(result.payload["query"], "hello");

    // Receipt was produced
    assert_eq!(execution.receipt.tool_name, "ping");
    assert!(execution.receipt.error_class.is_none());
    assert_eq!(execution.receipt.retry_owner, ToolRetryOwner::LlmPipeline);
}

// ===========================================================================
// 14. Runtime: execute tool with missing provider (unknown tool)
// ===========================================================================

#[tokio::test]
async fn test_runtime_execute_missing_provider() {
    let runtime = ToolRuntime::new(ToolRegistry::new());
    let call = ToolCall::new("nonexistent_tool", "1.0.0", json!({}), ToolOriginKind::Test);

    let execution = runtime.execute(&tool_ctx(), &call, None, None).await;
    let err = execution.result.unwrap_err();

    assert_eq!(err.class, ToolErrorClass::UnknownTool);
    assert!(err.message.contains("nonexistent_tool"));
    assert_eq!(
        execution.receipt.error_class,
        Some(ToolErrorClass::UnknownTool)
    );
    assert_eq!(execution.receipt.tool_name, "nonexistent_tool");
}

// ===========================================================================
// 15. Runtime: execute tool timeout behavior
// ===========================================================================

#[tokio::test]
async fn test_runtime_execute_timeout_behavior() {
    struct SlowTool {
        descriptor: ToolDescriptor,
    }

    #[async_trait]
    impl Tool for SlowTool {
        fn descriptor(&self) -> &ToolDescriptor {
            &self.descriptor
        }
        async fn invoke(&self, _ctx: &ToolCtx, _call: &ToolCall) -> Result<ToolResult, ToolError> {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            Ok(ToolResult::json(json!({"done": true})))
        }
    }

    let mut desc = read_only_descriptor("slow_tool");
    desc.timeout_ms = 1; // 1ms timeout — will always expire
    let mut registry = ToolRegistry::new();
    registry.register(SlowTool { descriptor: desc });
    let runtime = ToolRuntime::new(registry);

    let call = ToolCall::new(
        "slow_tool",
        "1.0.0",
        json!({"query": "waiting"}),
        ToolOriginKind::Test,
    );

    let execution = runtime.execute(&tool_ctx(), &call, None, None).await;
    let err = execution.result.unwrap_err();

    assert_eq!(err.class, ToolErrorClass::Timeout);
    assert!(err.message.contains("timed out"));
    assert_eq!(execution.receipt.error_class, Some(ToolErrorClass::Timeout));
}

// ===========================================================================
// 16. Contract validation: valid contract passes
// ===========================================================================

#[test]
fn test_contract_validation_valid() {
    let schema = json!({
        "type": "object",
        "required": ["query"],
        "properties": {
            "query": {"type": "string"},
            "limit": {"type": "integer"}
        },
        "additionalProperties": false
    });

    let valid_args = json!({"query": "search term", "limit": 5});
    validate_arguments_against_schema(&schema, &valid_args).unwrap();

    // Required-only also passes
    let minimal_args = json!({"query": "just the query"});
    validate_arguments_against_schema(&schema, &minimal_args).unwrap();
}

// ===========================================================================
// 17. Contract validation: empty name / missing required field fails
// ===========================================================================

#[test]
fn test_contract_validation_empty_name() {
    let schema = json!({
        "type": "object",
        "required": ["name"],
        "properties": {
            "name": {"type": "string"}
        },
        "additionalProperties": false
    });

    // Missing required field entirely
    let err = validate_arguments_against_schema(&schema, &json!({})).unwrap_err();
    assert_eq!(err.class, ToolErrorClass::InvalidArguments);
    assert!(
        err.message.contains("missing required field name"),
        "Expected 'missing required field name', got: {}",
        err.message
    );

    // Wrong type for the field
    let err = validate_arguments_against_schema(&schema, &json!({"name": 42})).unwrap_err();
    assert_eq!(err.class, ToolErrorClass::InvalidArguments);
    assert!(err.message.contains("expected string"));
}

// ===========================================================================
// 18. Contract validation: additional properties rejected
// ===========================================================================

#[test]
fn test_contract_validation_additional_properties_rejected() {
    let schema = json!({
        "type": "object",
        "required": ["query"],
        "properties": {
            "query": {"type": "string"}
        },
        "additionalProperties": false
    });

    let err = validate_arguments_against_schema(
        &schema,
        &json!({"query": "ok", "unexpected_field": true}),
    )
    .unwrap_err();

    assert_eq!(err.class, ToolErrorClass::InvalidArguments);
    assert!(err.message.contains("unexpected field"));
}

// ===========================================================================
// 19. Registry: plan_exposure filters hidden tools
// ===========================================================================

#[test]
fn test_registry_plan_exposure_filters_hidden() {
    let mut registry = ToolRegistry::new();

    let mut visible_desc = read_only_descriptor("visible_tool");
    visible_desc.exposure_mode = ToolExposureMode::Auto;
    registry.register(EchoTool {
        descriptor: visible_desc,
    });

    let mut hidden_desc = read_only_descriptor("hidden_tool");
    hidden_desc.exposure_mode = ToolExposureMode::Hidden;
    registry.register(EchoTool {
        descriptor: hidden_desc,
    });

    let request = ToolExposureRequest {
        allowed_names: None,
        planner_stage: ToolPlannerStage::Execution,
        include_hidden: false,
        max_tools: None,
    };
    let plan = registry.plan_exposure(&request);

    assert_eq!(plan.tools.len(), 1);
    assert_eq!(plan.tools[0].descriptor().name, "visible_tool");

    let hidden_decision = plan
        .decisions
        .iter()
        .find(|d| d.tool_name == "hidden_tool")
        .unwrap();
    assert!(!hidden_decision.exposed);
    assert_eq!(hidden_decision.reason, "hidden");
}

// ===========================================================================
// 20. Runtime: version mismatch produces ProviderContract error
// ===========================================================================

#[tokio::test]
async fn test_runtime_version_mismatch() {
    let mut registry = ToolRegistry::new();
    registry.register(EchoTool {
        descriptor: read_only_descriptor("versioned"),
    });
    let runtime = ToolRuntime::new(registry);

    let call = ToolCall::new(
        "versioned",
        "9.9.9", // wrong version
        json!({"query": "test"}),
        ToolOriginKind::Test,
    );

    let execution = runtime.execute(&tool_ctx(), &call, None, None).await;
    let err = execution.result.unwrap_err();

    assert_eq!(err.class, ToolErrorClass::ProviderContract);
    assert!(err.message.contains("version mismatch"));
    assert_eq!(
        execution.receipt.error_class,
        Some(ToolErrorClass::ProviderContract)
    );
}

// ===========================================================================
// 21. Runtime: InMemoryReceiptSink persists and retrieves receipts
// ===========================================================================

#[tokio::test]
async fn test_in_memory_receipt_sink() {
    let sink = Arc::new(InMemoryReceiptSink::default());
    let receipt = make_receipt("echo", ToolRetryOwner::LlmPipeline, None);

    sink.persist(&receipt).await.unwrap();
    sink.persist(&receipt).await.unwrap();

    let stored = sink.receipts();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].tool_name, "echo");
    assert_eq!(stored[1].tool_name, "echo");
}

// ===========================================================================
// 22. Runtime: failing tool invocation propagates error through receipt
// ===========================================================================

#[tokio::test]
async fn test_runtime_failing_tool_error_propagation() {
    let mut registry = ToolRegistry::new();
    registry.register(FailingTool {
        descriptor: read_only_descriptor("always_fails"),
    });
    let runtime = ToolRuntime::new(registry);

    let call = ToolCall::new(
        "always_fails",
        "1.0.0",
        json!({"query": "will fail"}),
        ToolOriginKind::Test,
    );

    let execution = runtime.execute(&tool_ctx(), &call, None, None).await;
    let err = execution.result.unwrap_err();

    assert_eq!(err.class, ToolErrorClass::Execution);
    assert_eq!(err.message, "intentional test failure");
    assert_eq!(
        execution.receipt.error_class,
        Some(ToolErrorClass::Execution)
    );
    assert_eq!(execution.receipt.tool_name, "always_fails");
}
