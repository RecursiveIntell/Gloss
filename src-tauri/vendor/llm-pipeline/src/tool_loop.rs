use crate::{
    client::LlmConfig, exec_ctx::ExecCtx, streaming::StreamingDecoder, PipelineError, Result,
};
use futures::StreamExt;
use llm_tool_runtime::{
    render_ollama_tool, render_openai_tool, ApprovalGrant, ToolBudgetContext, ToolCall, ToolCtx,
    ToolDescriptor, ToolError, ToolExecutionPermit, ToolExposureDecision, ToolExposureRequest,
    ToolOriginKind, ToolPlannerStage, ToolReceiptSink, ToolRegistry, ToolResult, ToolRetryOwner,
    ToolRuntime,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use stack_ids::{AttemptId, ScopeKey, TraceCtx, TrialId};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

type JsonFetchFuture<'a> = Pin<Box<dyn Future<Output = Result<Value>> + 'a>>;
type JsonFetcher<'a> = dyn FnMut(Value) -> JsonFetchFuture<'a> + 'a;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolLoopChoice {
    Auto,
    None,
    Required { tool_name: String },
}

#[derive(Debug, Clone)]
pub struct ToolLoopRequest {
    pub model: String,
    pub user_input: String,
    pub instructions: Option<String>,
    pub config: LlmConfig,
    pub attempt_id: Option<AttemptId>,
    pub planner_stage: ToolPlannerStage,
    pub retry_owner: ToolRetryOwner,
    pub allowed_tools: Option<Vec<String>>,
    pub tool_choice: ToolLoopChoice,
    pub max_round_trips: u32,
    pub strict: bool,
    pub parallel_tool_calls: bool,
    pub dry_run: bool,
    pub scope: Option<ScopeKey>,
    pub approval_grant: Option<ApprovalGrant>,
    pub execution_permit: Option<std::sync::Arc<ToolExecutionPermit>>,
    pub stream: bool,
    pub api_key: Option<String>,
    pub organization: Option<String>,
}

impl ToolLoopRequest {
    pub fn new(model: impl Into<String>, user_input: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            user_input: user_input.into(),
            instructions: None,
            config: LlmConfig::default(),
            attempt_id: None,
            planner_stage: ToolPlannerStage::Execution,
            retry_owner: ToolRetryOwner::LlmPipeline,
            allowed_tools: None,
            tool_choice: ToolLoopChoice::Auto,
            max_round_trips: 8,
            strict: true,
            parallel_tool_calls: false,
            dry_run: false,
            scope: None,
            approval_grant: None,
            execution_permit: None,
            stream: false,
            api_key: None,
            organization: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub call: ToolCall,
    pub outcome: std::result::Result<ToolResult, ToolError>,
    pub receipt: llm_tool_runtime::ToolReceipt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolLoopResponse {
    pub final_text: String,
    pub rounds: u32,
    pub trace_ctx: TraceCtx,
    pub invocations: Vec<ToolInvocation>,
    pub exposure_decisions: Vec<ToolExposureDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_response_id: Option<String>,
}

pub struct ToolLoopRunner {
    runtime: Arc<ToolRuntime>,
}

struct PreparedToolLoop {
    descriptors_by_name: BTreeMap<String, ToolDescriptor>,
    tools: Vec<Value>,
    exposure_decisions: Vec<ToolExposureDecision>,
}

impl ToolLoopRunner {
    pub fn new(runtime: Arc<ToolRuntime>) -> Self {
        Self { runtime }
    }

    pub fn from_registry(registry: ToolRegistry) -> Self {
        Self::new(Arc::new(ToolRuntime::new(registry)))
    }

    pub fn from_registry_with_receipt_sink(
        registry: ToolRegistry,
        receipt_sink: Arc<dyn ToolReceiptSink>,
    ) -> Self {
        Self::new(Arc::new(
            ToolRuntime::new(registry).with_receipt_sink(receipt_sink),
        ))
    }

    pub fn runtime(&self) -> &Arc<ToolRuntime> {
        &self.runtime
    }

    fn prepare_tool_loop<F>(
        &self,
        request: &ToolLoopRequest,
        mut render_tool: F,
    ) -> Result<PreparedToolLoop>
    where
        F: FnMut(&ToolDescriptor) -> std::result::Result<Value, ToolError>,
    {
        let exposure = self.runtime.registry().plan_exposure(&ToolExposureRequest {
            allowed_names: request.allowed_tools.clone(),
            planner_stage: request.planner_stage.clone(),
            include_hidden: false,
            max_tools: None,
        });
        let descriptors = exposure
            .tools
            .iter()
            .map(|tool| tool.descriptor().clone())
            .collect::<Vec<_>>();
        let tools = descriptors
            .iter()
            .map(&mut render_tool)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(tool_error_to_pipeline)?;
        let descriptors_by_name = descriptors
            .into_iter()
            .map(|descriptor| (descriptor.name.clone(), descriptor))
            .collect::<BTreeMap<_, _>>();

        Ok(PreparedToolLoop {
            descriptors_by_name,
            tools,
            exposure_decisions: exposure.decisions,
        })
    }

    async fn execute_tool_call(
        &self,
        ctx: &ExecCtx,
        request: &ToolLoopRequest,
        attempt_id: &AttemptId,
        call: ToolCall,
        invocations: &mut Vec<ToolInvocation>,
    ) -> String {
        let tool_ctx = build_tool_ctx(ctx, request, attempt_id);
        let execution = self
            .runtime
            .execute(
                &tool_ctx,
                &call,
                request.execution_permit.clone(),
                ctx.cancel_flag(),
            )
            .await;
        let tool_output = execution
            .result
            .as_ref()
            .map(ToolResult::to_model_output)
            .unwrap_or_else(|error| {
                json!({
                    "error_class": error.class,
                    "message": error.message,
                })
                .to_string()
            });
        invocations.push(ToolInvocation {
            call,
            outcome: execution.result,
            receipt: execution.receipt,
        });
        tool_output
    }

    async fn execute_openai_function_calls(
        &self,
        ctx: &ExecCtx,
        request: &ToolLoopRequest,
        attempt_id: &AttemptId,
        descriptors_by_name: &BTreeMap<String, ToolDescriptor>,
        function_calls: Vec<OpenAiFunctionCall>,
        invocations: &mut Vec<ToolInvocation>,
    ) -> Result<Vec<Value>> {
        let mut outputs = Vec::with_capacity(function_calls.len());
        for function_call in function_calls {
            let descriptor = descriptors_by_name
                .get(&function_call.name)
                .ok_or_else(|| {
                    PipelineError::Other(format!(
                        "provider requested unknown or unexposed tool {}",
                        function_call.name
                    ))
                })?;
            let call = ToolCall {
                descriptor_name: descriptor.name.clone(),
                descriptor_version: descriptor.version.clone(),
                arguments: function_call.arguments.clone(),
                origin_kind: ToolOriginKind::OpenAiResponses,
                provider_call_id: Some(function_call.call_id.clone()),
                tool_run_id: uuid::Uuid::new_v4().to_string(),
            };
            let tool_output = self
                .execute_tool_call(ctx, request, attempt_id, call, invocations)
                .await;
            outputs.push(json!({
                "type": "function_call_output",
                "call_id": function_call.call_id,
                "output": tool_output,
            }));
        }
        Ok(outputs)
    }

    async fn execute_ollama_tool_calls(
        &self,
        ctx: &ExecCtx,
        request: &ToolLoopRequest,
        attempt_id: &AttemptId,
        descriptors_by_name: &BTreeMap<String, ToolDescriptor>,
        tool_calls: Vec<OllamaFunctionCall>,
        invocations: &mut Vec<ToolInvocation>,
    ) -> Result<Vec<Value>> {
        let mut outputs = Vec::with_capacity(tool_calls.len());
        for tool_call in tool_calls {
            let descriptor = descriptors_by_name.get(&tool_call.name).ok_or_else(|| {
                PipelineError::Other(format!(
                    "provider requested unknown or unexposed tool {}",
                    tool_call.name
                ))
            })?;
            let call = ToolCall {
                descriptor_name: descriptor.name.clone(),
                descriptor_version: descriptor.version.clone(),
                arguments: tool_call.arguments.clone(),
                origin_kind: ToolOriginKind::OllamaChat,
                provider_call_id: None,
                tool_run_id: uuid::Uuid::new_v4().to_string(),
            };
            let tool_output = self
                .execute_tool_call(ctx, request, attempt_id, call, invocations)
                .await;
            outputs.push(json!({
                "role": "tool",
                "name": tool_call.name,
                "content": tool_output,
            }));
        }
        Ok(outputs)
    }

    pub async fn run_openai_responses(
        &self,
        ctx: &ExecCtx,
        request: ToolLoopRequest,
    ) -> Result<ToolLoopResponse> {
        let api_key = request.api_key.clone();
        let organization = request.organization.clone();
        let mut fetch = |body: Value| -> JsonFetchFuture<'_> {
            let api_key = api_key.clone();
            let organization = organization.clone();
            Box::pin(async move {
                post_json_with_backoff(
                    ctx,
                    "/v1/responses",
                    &body,
                    api_key.as_deref(),
                    organization.as_deref(),
                )
                .await
            })
        };
        self.run_openai_responses_with_fetcher(ctx, request, &mut fetch)
            .await
    }

    async fn run_openai_responses_with_fetcher(
        &self,
        ctx: &ExecCtx,
        request: ToolLoopRequest,
        fetch: &mut JsonFetcher<'_>,
    ) -> Result<ToolLoopResponse> {
        let prepared = self.prepare_tool_loop(&request, |descriptor| {
            render_openai_tool(descriptor, request.strict)
        })?;

        let mut input_items = vec![json!({
            "role": "user",
            "content": [{"type": "input_text", "text": request.user_input}],
        })];
        let mut invocations = Vec::new();
        let attempt_id = request
            .attempt_id
            .clone()
            .unwrap_or_else(AttemptId::generate);

        for round in 0..request.max_round_trips {
            ctx.check_cancelled()?;
            let response =
                fetch(openai_request_body(&request, &input_items, &prepared.tools)).await?;

            let provider_response_id = response
                .get("id")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string());

            let output_items = response
                .get("output")
                .and_then(|value| value.as_array())
                .cloned()
                .unwrap_or_default();
            let function_calls = parse_openai_function_calls(&output_items)?;

            if function_calls.is_empty() {
                return Ok(ToolLoopResponse {
                    final_text: openai_output_text(&response),
                    rounds: round + 1,
                    trace_ctx: ctx.trace_ctx.clone(),
                    invocations,
                    exposure_decisions: prepared.exposure_decisions.clone(),
                    provider_response_id,
                });
            }

            if function_calls.len() > 1 && !request.parallel_tool_calls {
                return Err(PipelineError::InvalidConfig(
                    "parallel tool calls are disabled by default".into(),
                ));
            }

            input_items.extend(output_items);
            input_items.extend(
                self.execute_openai_function_calls(
                    ctx,
                    &request,
                    &attempt_id,
                    &prepared.descriptors_by_name,
                    function_calls,
                    &mut invocations,
                )
                .await?,
            );
        }

        Err(PipelineError::Other(format!(
            "tool loop exceeded max_round_trips={}",
            request.max_round_trips
        )))
    }

    pub async fn run_ollama(
        &self,
        ctx: &ExecCtx,
        request: ToolLoopRequest,
    ) -> Result<ToolLoopResponse> {
        if request.stream {
            return self.run_ollama_streaming(ctx, request).await;
        }

        let mut fetch = |body: Value| -> JsonFetchFuture<'_> {
            Box::pin(
                async move { post_json_with_backoff(ctx, "/api/chat", &body, None, None).await },
            )
        };
        self.run_ollama_with_fetcher(ctx, request, &mut fetch).await
    }

    async fn run_ollama_with_fetcher(
        &self,
        ctx: &ExecCtx,
        request: ToolLoopRequest,
        fetch: &mut JsonFetcher<'_>,
    ) -> Result<ToolLoopResponse> {
        let prepared = self.prepare_tool_loop(&request, render_ollama_tool)?;

        let mut messages = Vec::new();
        if let Some(instructions) = &request.instructions {
            messages.push(json!({"role": "system", "content": instructions}));
        }
        messages.push(json!({"role": "user", "content": request.user_input}));

        let mut invocations = Vec::new();
        let attempt_id = request
            .attempt_id
            .clone()
            .unwrap_or_else(AttemptId::generate);
        for round in 0..request.max_round_trips {
            ctx.check_cancelled()?;
            let response = fetch(ollama_request_body(
                &request,
                &messages,
                &prepared.tools,
                false,
            ))
            .await?;

            let message = response
                .get("message")
                .cloned()
                .ok_or_else(|| PipelineError::Other("Ollama response missing message".into()))?;
            let tool_calls = parse_ollama_tool_calls(&message)?;
            messages.push(message.clone());

            if tool_calls.is_empty() {
                return Ok(ToolLoopResponse {
                    final_text: message
                        .get("content")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    rounds: round + 1,
                    trace_ctx: ctx.trace_ctx.clone(),
                    invocations,
                    exposure_decisions: prepared.exposure_decisions.clone(),
                    provider_response_id: None,
                });
            }

            if tool_calls.len() > 1 && !request.parallel_tool_calls {
                return Err(PipelineError::InvalidConfig(
                    "parallel tool calls are disabled by default".into(),
                ));
            }

            messages.extend(
                self.execute_ollama_tool_calls(
                    ctx,
                    &request,
                    &attempt_id,
                    &prepared.descriptors_by_name,
                    tool_calls,
                    &mut invocations,
                )
                .await?,
            );
        }

        Err(PipelineError::Other(format!(
            "tool loop exceeded max_round_trips={}",
            request.max_round_trips
        )))
    }

    async fn run_ollama_streaming(
        &self,
        ctx: &ExecCtx,
        request: ToolLoopRequest,
    ) -> Result<ToolLoopResponse> {
        let prepared = self.prepare_tool_loop(&request, render_ollama_tool)?;

        let mut messages = Vec::new();
        if let Some(instructions) = &request.instructions {
            messages.push(json!({"role": "system", "content": instructions}));
        }
        messages.push(json!({"role": "user", "content": request.user_input}));

        let mut invocations = Vec::new();
        let attempt_id = request
            .attempt_id
            .clone()
            .unwrap_or_else(AttemptId::generate);
        for round in 0..request.max_round_trips {
            ctx.check_cancelled()?;
            let response = post_ollama_stream(ctx, &request, &messages, &prepared.tools).await?;

            let message = response
                .get("message")
                .cloned()
                .ok_or_else(|| PipelineError::Other("Ollama response missing message".into()))?;
            let tool_calls = parse_ollama_tool_calls(&message)?;
            messages.push(message.clone());

            if tool_calls.is_empty() {
                return Ok(ToolLoopResponse {
                    final_text: message
                        .get("content")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    rounds: round + 1,
                    trace_ctx: ctx.trace_ctx.clone(),
                    invocations,
                    exposure_decisions: prepared.exposure_decisions.clone(),
                    provider_response_id: None,
                });
            }

            if tool_calls.len() > 1 && !request.parallel_tool_calls {
                return Err(PipelineError::InvalidConfig(
                    "parallel tool calls are disabled by default".into(),
                ));
            }

            messages.extend(
                self.execute_ollama_tool_calls(
                    ctx,
                    &request,
                    &attempt_id,
                    &prepared.descriptors_by_name,
                    tool_calls,
                    &mut invocations,
                )
                .await?,
            );
        }

        Err(PipelineError::Other(format!(
            "tool loop exceeded max_round_trips={}",
            request.max_round_trips
        )))
    }
}

#[derive(Debug, Clone)]
struct OpenAiFunctionCall {
    name: String,
    call_id: String,
    arguments: Value,
}

#[derive(Debug, Clone)]
struct OllamaFunctionCall {
    name: String,
    arguments: Value,
}

fn build_tool_ctx(ctx: &ExecCtx, request: &ToolLoopRequest, attempt_id: &AttemptId) -> ToolCtx {
    let deadline = chrono::Utc::now()
        + chrono::Duration::from_std(ctx.limits.request_timeout)
            .unwrap_or_else(|_| chrono::Duration::seconds(60));
    ToolCtx {
        trace_ctx: ctx.trace_ctx.clone(),
        attempt_id: attempt_id.clone(),
        trial_id: TrialId::generate(),
        deadline: Some(deadline.to_rfc3339()),
        workload_class: Some("llm_pipeline_tool_loop".into()),
        budget_context: Some(ToolBudgetContext {
            budget_kind: Some("tool_loop".into()),
            max_steps: Some(request.max_round_trips),
            time_budget_ms: Some(ctx.limits.request_timeout.as_millis() as u64),
            cost_budget_units: None,
        }),
        scope: request.scope.clone(),
        dry_run: request.dry_run,
        approval_grant: request.approval_grant.clone(),
        execution_permit: request.execution_permit.clone(),
        idempotency_key: None,
        caller: format!("llm-pipeline:{}", request.model),
        planner_stage: request.planner_stage.clone(),
        parent_receipt_id: None,
        family_receipt_id: None,
        replay_parent_receipt_id: None,
        remote_oracle_lease_id: None,
        remote_slice_result_id: None,
        attestation_envelope_id: None,
        cross_runtime_replay_ticket_id: None,
        retry_owner: Some(request.retry_owner.clone()),
    }
}

fn openai_request_body(request: &ToolLoopRequest, input_items: &[Value], tools: &[Value]) -> Value {
    let mut body = json!({
        "model": request.model,
        "input": input_items,
        "parallel_tool_calls": request.parallel_tool_calls,
    });

    if let Some(instructions) = &request.instructions {
        body["instructions"] = json!(instructions);
    }

    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
        body["tool_choice"] = match &request.tool_choice {
            ToolLoopChoice::Auto => json!("auto"),
            ToolLoopChoice::None => json!("none"),
            ToolLoopChoice::Required { tool_name } => json!({
                "type": "function",
                "name": tool_name,
            }),
        };
    }

    body
}

fn ollama_request_body(
    request: &ToolLoopRequest,
    messages: &[Value],
    tools: &[Value],
    stream: bool,
) -> Value {
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": stream,
        "options": {
            "temperature": request.config.temperature,
            "num_predict": request.config.max_tokens,
        }
    });

    if request.config.thinking {
        body["think"] = json!(true);
    }
    if request.config.json_mode {
        body["format"] = json!("json");
    }
    if let Some(options) = request.config.options.clone() {
        body["options"] = merge_json_objects(body["options"].clone(), options);
    }
    if !tools.is_empty() && !matches!(request.tool_choice, ToolLoopChoice::None) {
        body["tools"] = Value::Array(tools.to_vec());
    }

    body
}

async fn post_json_with_backoff(
    ctx: &ExecCtx,
    path: &str,
    body: &Value,
    api_key: Option<&str>,
    organization: Option<&str>,
) -> Result<Value> {
    let url = format!("{}{}", ctx.base_url.trim_end_matches('/'), path);
    let mut attempt = 0u32;

    loop {
        ctx.check_cancelled()?;
        let mut request = ctx.client.post(&url).json(body);
        if let Some(api_key) = api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }
        if let Some(organization) = organization {
            request = request.header("OpenAI-Organization", organization);
        }
        let response = request.send().await.map_err(PipelineError::Request)?;

        if response.status().is_success() {
            return response.json().await.map_err(PipelineError::Request);
        }

        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_secs);
        let error = PipelineError::HttpError {
            status: response.status().as_u16(),
            body: response.text().await.unwrap_or_default(),
            retry_after,
        };

        if attempt >= ctx.backoff.max_retries || !crate::backend::is_retryable(&error, &ctx.backoff)
        {
            return Err(error);
        }

        let delay = retry_after
            .filter(|_| ctx.backoff.respect_retry_after)
            .unwrap_or_else(|| ctx.backoff.delay_for_attempt(attempt));
        tokio::time::sleep(delay).await;
        if let Some(flag) = ctx.cancel_flag() {
            if flag.load(Ordering::Relaxed) {
                return Err(PipelineError::Cancelled);
            }
        }
        attempt += 1;
    }
}

async fn post_ollama_stream(
    ctx: &ExecCtx,
    request: &ToolLoopRequest,
    messages: &[Value],
    tools: &[Value],
) -> Result<Value> {
    let url = format!("{}/api/chat", ctx.base_url.trim_end_matches('/'));
    let response = ctx
        .client
        .post(&url)
        .json(&ollama_request_body(request, messages, tools, true))
        .send()
        .await
        .map_err(PipelineError::Request)?;

    if !response.status().is_success() {
        return Err(PipelineError::HttpError {
            status: response.status().as_u16(),
            body: response.text().await.unwrap_or_default(),
            retry_after: None,
        });
    }

    let stream = response.bytes_stream().map(|chunk| {
        chunk
            .map(|chunk| chunk.to_vec())
            .map_err(PipelineError::Request)
    });

    consume_ollama_stream(ctx, stream).await
}

async fn consume_ollama_stream<S>(ctx: &ExecCtx, mut stream: S) -> Result<Value>
where
    S: futures::Stream<Item = Result<Vec<u8>>> + Unpin,
{
    let mut decoder = StreamingDecoder::new();
    let mut content = String::new();
    let mut thinking = String::new();
    let mut tool_calls = Vec::new();

    loop {
        ctx.check_cancelled()?;
        let Some(chunk) = stream.next().await else {
            break;
        };
        let chunk = chunk?;
        for value in decoder.decode(&chunk) {
            accumulate_ollama_stream_message(&value, &mut content, &mut thinking, &mut tool_calls);
        }
    }
    ctx.check_cancelled()?;
    if let Some(value) = decoder.flush() {
        accumulate_ollama_stream_message(&value, &mut content, &mut thinking, &mut tool_calls);
    }

    Ok(json!({
        "message": {
            "role": "assistant",
            "content": content,
            "thinking": thinking,
            "tool_calls": tool_calls,
        }
    }))
}

fn accumulate_ollama_stream_message(
    value: &Value,
    content: &mut String,
    thinking: &mut String,
    tool_calls: &mut Vec<Value>,
) {
    let Some(message) = value.get("message") else {
        return;
    };
    if let Some(partial_content) = message.get("content").and_then(|value| value.as_str()) {
        content.push_str(partial_content);
    }
    if let Some(partial_thinking) = message.get("thinking").and_then(|value| value.as_str()) {
        thinking.push_str(partial_thinking);
    }
    if let Some(partial_tool_calls) = message.get("tool_calls").and_then(|value| value.as_array()) {
        tool_calls.extend(partial_tool_calls.iter().cloned());
    }
}

fn parse_openai_function_calls(output_items: &[Value]) -> Result<Vec<OpenAiFunctionCall>> {
    let mut calls = Vec::new();
    for item in output_items {
        if item.get("type").and_then(|value| value.as_str()) != Some("function_call") {
            continue;
        }

        let name = item
            .get("name")
            .and_then(|value| value.as_str())
            .ok_or_else(|| PipelineError::Other("OpenAI function call missing name".into()))?;
        let call_id = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(|value| value.as_str())
            .ok_or_else(|| PipelineError::Other("OpenAI function call missing call_id".into()))?;
        let arguments = match item.get("arguments") {
            Some(Value::String(arguments)) if arguments.trim().is_empty() => {
                Value::Object(Default::default())
            }
            Some(Value::String(arguments)) => serde_json::from_str(arguments)
                .map_err(|error| PipelineError::Other(error.to_string()))?,
            Some(arguments) => arguments.clone(),
            None => Value::Object(Default::default()),
        };

        calls.push(OpenAiFunctionCall {
            name: name.to_string(),
            call_id: call_id.to_string(),
            arguments,
        });
    }

    Ok(calls)
}

fn parse_ollama_tool_calls(message: &Value) -> Result<Vec<OllamaFunctionCall>> {
    let raw_calls = message
        .get("tool_calls")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let mut calls = Vec::new();

    for item in raw_calls {
        let function = item
            .get("function")
            .ok_or_else(|| PipelineError::Other("Ollama tool call missing function".into()))?;
        let name = function
            .get("name")
            .and_then(|value| value.as_str())
            .ok_or_else(|| PipelineError::Other("Ollama tool call missing name".into()))?;
        let arguments = function
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));
        calls.push(OllamaFunctionCall {
            name: name.to_string(),
            arguments,
        });
    }

    Ok(calls)
}

fn openai_output_text(response: &Value) -> String {
    if let Some(output_text) = response.get("output_text").and_then(|value| value.as_str()) {
        if !output_text.is_empty() {
            return output_text.to_string();
        }
    }

    let mut rendered = String::new();
    if let Some(items) = response.get("output").and_then(|value| value.as_array()) {
        for item in items {
            if item.get("type").and_then(|value| value.as_str()) != Some("message") {
                continue;
            }
            if let Some(content) = item.get("content").and_then(|value| value.as_array()) {
                for part in content {
                    if let Some(text) = part.get("text").and_then(|value| value.as_str()) {
                        rendered.push_str(text);
                    }
                }
            }
        }
    }
    rendered
}

fn merge_json_objects(left: Value, right: Value) -> Value {
    let mut merged = left.as_object().cloned().unwrap_or_default();
    for (key, value) in right.as_object().cloned().unwrap_or_default() {
        merged.insert(key, value);
    }
    Value::Object(merged)
}

fn tool_error_to_pipeline(error: ToolError) -> PipelineError {
    PipelineError::Other(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use forge_engine::{ForgeStore, ForgeToolReceiptSink};
    use llm_tool_runtime::{
        Tool, ToolApprovalKind, ToolBackendKind, ToolDescriptor, ToolExposureMode,
        ToolExposurePolicy, ToolIdempotencyClass, ToolOutputMode, ToolReceiptPersistence,
        ToolSideEffectClass,
    };
    use std::collections::VecDeque;

    #[derive(Clone)]
    struct AddTool {
        descriptor: ToolDescriptor,
    }

    #[async_trait]
    impl Tool for AddTool {
        fn descriptor(&self) -> &ToolDescriptor {
            &self.descriptor
        }

        async fn invoke(
            &self,
            _ctx: &ToolCtx,
            call: &ToolCall,
        ) -> std::result::Result<ToolResult, ToolError> {
            let a = call
                .arguments
                .get("a")
                .and_then(|value| value.as_i64())
                .unwrap_or(0);
            let b = call
                .arguments
                .get("b")
                .and_then(|value| value.as_i64())
                .unwrap_or(0);
            Ok(ToolResult::text((a + b).to_string()))
        }
    }

    fn add_descriptor() -> ToolDescriptor {
        ToolDescriptor {
            name: "add".into(),
            version: "1.0.0".into(),
            description: Some("Add two numbers".into()),
            backend_kind: ToolBackendKind::LocalFunction,
            input_schema: json!({
                "type": "object",
                "required": ["a", "b"],
                "properties": {
                    "a": {"type": "integer"},
                    "b": {"type": "integer"}
                },
                "additionalProperties": false
            }),
            output_mode: ToolOutputMode::Text,
            read_only: true,
            side_effect_class: ToolSideEffectClass::ReadOnly,
            idempotency_class: ToolIdempotencyClass::Idempotent,
            approval_kind: ToolApprovalKind::None,
            timeout_ms: 5_000,
            concurrency_key: None,
            cache_ttl_ms: None,
            exposure_mode: ToolExposureMode::Auto,
            mcp_surface_kind: llm_tool_runtime::McpSurfaceKind::Tool,
            exposure_policy: ToolExposurePolicy::default(),
            receipt_persistence: ToolReceiptPersistence::Ephemeral,
            output_size_limit_bytes: None,
            provider_payload: None,
            effect_target: Default::default(),
            rollback_contract: None,
        }
    }

    fn runner() -> ToolLoopRunner {
        let mut registry = llm_tool_runtime::ToolRegistry::new();
        registry.register(AddTool {
            descriptor: add_descriptor(),
        });
        ToolLoopRunner::from_registry(registry)
    }

    fn durable_runner(store: Arc<ForgeStore>) -> ToolLoopRunner {
        let mut registry = llm_tool_runtime::ToolRegistry::new();
        let mut descriptor = add_descriptor();
        descriptor.receipt_persistence = ToolReceiptPersistence::ForgeRaw;
        registry.register(AddTool { descriptor });
        ToolLoopRunner::from_registry_with_receipt_sink(
            registry,
            Arc::new(ForgeToolReceiptSink::new(store)),
        )
    }

    fn canned_fetcher(responses: Vec<Value>) -> impl FnMut(Value) -> JsonFetchFuture<'static> {
        let mut responses = VecDeque::from(responses);
        move |_body: Value| {
            let response = responses
                .pop_front()
                .expect("canned provider response should be available");
            Box::pin(async move { Ok(response) })
        }
    }

    #[tokio::test]
    async fn openai_tool_loop_executes_local_function_calls() {
        let ctx = ExecCtx::builder("http://provider.invalid").build();
        let mut fetch = canned_fetcher(vec![
            json!({
                "id": "resp_1",
                "output": [
                    {
                        "type": "function_call",
                        "call_id": "call_1",
                        "name": "add",
                        "arguments": "{\"a\":2,\"b\":3}"
                    }
                ]
            }),
            json!({
                "id": "resp_2",
                "output_text": "5"
            }),
        ]);
        let output = runner()
            .run_openai_responses_with_fetcher(
                &ctx,
                ToolLoopRequest::new("gpt-4.1", "add 2 and 3"),
                &mut fetch,
            )
            .await
            .unwrap();

        assert_eq!(output.final_text, "5");
        assert_eq!(output.invocations.len(), 1);
    }

    #[tokio::test]
    async fn ollama_tool_loop_executes_local_function_calls() {
        let ctx = ExecCtx::builder("http://provider.invalid").build();
        let mut fetch = canned_fetcher(vec![
            json!({
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [
                        {
                            "type": "function",
                            "function": {
                                "index": 0,
                                "name": "add",
                                "arguments": {"a": 4, "b": 6}
                            }
                        }
                    ]
                }
            }),
            json!({
                "message": {
                    "role": "assistant",
                    "content": "10"
                }
            }),
        ]);
        let output = runner()
            .run_ollama_with_fetcher(
                &ctx,
                ToolLoopRequest::new("qwen3", "add 4 and 6"),
                &mut fetch,
            )
            .await
            .unwrap();

        assert_eq!(output.final_text, "10");
        assert_eq!(output.invocations.len(), 1);
    }

    #[tokio::test]
    async fn openai_tool_loop_persists_forge_raw_receipts_via_receipt_sink() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(ForgeStore::open(&dir.path().join("forge.db")).unwrap());
        let ctx = ExecCtx::builder("http://provider.invalid").build();
        let mut fetch = canned_fetcher(vec![
            json!({
                "id": "resp_receipt_1",
                "output": [
                    {
                        "type": "function_call",
                        "call_id": "call_receipt_1",
                        "name": "add",
                        "arguments": "{\"a\":1,\"b\":2}"
                    }
                ]
            }),
            json!({
                "id": "resp_receipt_2",
                "output_text": "3"
            }),
        ]);

        let output = durable_runner(store.clone())
            .run_openai_responses_with_fetcher(
                &ctx,
                ToolLoopRequest::new("gpt-4.1", "add 1 and 2"),
                &mut fetch,
            )
            .await
            .unwrap();

        assert_eq!(output.final_text, "3");
        assert_eq!(output.invocations.len(), 1);

        let receipt_id = &output.invocations[0].receipt.receipt_id;
        let stored = store
            .get_tool_receipt(receipt_id)
            .unwrap()
            .expect("receipt should persist to Forge");

        assert_eq!(stored.tool_name, "add");
        assert_eq!(stored.tool_version, "1.0.0");
        assert_eq!(stored.provider_call_id.as_deref(), Some("call_receipt_1"));
        assert_eq!(stored.trace_id, output.trace_ctx.trace_id);
    }

    #[tokio::test]
    async fn ollama_streaming_checks_cancellation_between_chunks() {
        use futures::stream::poll_fn;
        use std::task::Poll;

        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ctx = ExecCtx::builder("http://provider.invalid")
            .cancellation(Some(cancel.clone()))
            .build();
        let first = b"{\"message\":{\"content\":\"hello\"}}\n".to_vec();
        let second = b"{\"message\":{\"content\":\" world\"}}\n".to_vec();
        let cancel_for_stream = cancel.clone();
        let mut step = 0;

        let stream = poll_fn(move |_cx| match step {
            0 => {
                step = 1;
                cancel_for_stream.store(true, Ordering::Relaxed);
                Poll::Ready(Some(Ok(first.clone())))
            }
            1 => {
                step = 2;
                Poll::Ready(Some(Ok(second.clone())))
            }
            _ => Poll::Ready(None),
        });

        let err = consume_ollama_stream(&ctx, stream).await.unwrap_err();
        assert!(matches!(err, PipelineError::Cancelled));
    }
}
