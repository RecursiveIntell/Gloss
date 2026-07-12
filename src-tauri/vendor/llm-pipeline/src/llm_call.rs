//! LLM call payload — the primary execution unit.
//!
//! [`LlmCall`] is the primary payload for interacting with LLM providers.
//! It handles prompt rendering, backend dispatch (Ollama, OpenAI, etc.),
//! output parsing via [`OutputStrategy`], and optional semantic retry via
//! [`RetryConfig`].

use crate::{
    backend::{self, ChatMessage, LlmRequest, LlmResponse},
    client::LlmConfig,
    diagnostics::ParseDiagnostics,
    error::Result,
    events::{emit, Event},
    exec_ctx::ExecCtx,
    output_parser,
    output_strategy::OutputStrategy,
    parsing,
    payload::{BoxFut, Payload, PayloadOutput},
    retry::RetryConfig,
};
use llm_output_parser::ParseOptions;
use serde_json::{json, Value};
use stack_ids::{AttemptId, TrialId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant as StdInstant};
use tokio::sync::{mpsc, watch};

/// An LLM call payload that invokes a backend with output strategy and optional retry.
///
/// # Example
///
/// ```ignore
/// use llm_pipeline::{LlmCall, ExecCtx};
/// use llm_pipeline::payload::Payload;
/// use serde_json::json;
///
/// let call = LlmCall::new("summarize", "Summarize this: {input}")
///     .with_model("llama3.2:3b")
///     .with_config(LlmConfig::default().with_json_mode(true))
///     .expecting_json();
///
/// let ctx = ExecCtx::builder("http://localhost:11434").build();
/// let output = call.invoke(&ctx, json!("Some long text...")).await?;
/// ```
pub struct LlmCall {
    /// Instance name (for logging/events).
    name: String,
    /// Prompt template with `{input}` and `{key}` placeholders.
    prompt_template: String,
    /// Optional system prompt template (triggers chat endpoint on Ollama).
    system_template: Option<String>,
    /// Model identifier (e.g. `"llama3.2:3b"`).
    model: String,
    /// LLM configuration (temperature, tokens, json_mode, etc.).
    config: LlmConfig,
    /// Whether to use the streaming endpoint.
    streaming: bool,
    /// How to parse the raw LLM text into a Value. Default: `Lossy`.
    output_strategy: OutputStrategy,
    /// Optional semantic retry configuration.
    retry: Option<RetryConfig>,
    /// Optional per-request timeout for this payload's HTTP calls.
    ///
    /// When `Some`, this timeout is applied to each individual HTTP request
    /// via `reqwest::RequestBuilder::timeout()`. When `None`, the timeout
    /// falls back to [`PipelineLimits::request_timeout`](crate::limits::PipelineLimits::request_timeout)
    /// from the [`ExecCtx`].
    ///
    /// This enables mixed-latency payloads on the same context -- e.g., a
    /// 5 s classifier and a 120 s generator can each have their own timeout.
    timeout: Option<Duration>,
}

impl LlmCall {
    /// Create a new LLM call payload with a prompt template.
    pub fn new(name: impl Into<String>, prompt_template: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            prompt_template: prompt_template.into(),
            system_template: None,
            model: "llama3.2:3b".to_string(),
            config: LlmConfig::default(),
            streaming: false,
            output_strategy: OutputStrategy::default(),
            retry: None,
            timeout: None,
        }
    }

    /// Returns the prompt template.
    pub fn prompt_template(&self) -> &str {
        &self.prompt_template
    }

    /// Returns the system template, if any.
    pub fn system_template(&self) -> Option<&str> {
        self.system_template.as_deref()
    }

    /// Returns the model identifier.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Returns the LLM config.
    pub fn config(&self) -> &LlmConfig {
        &self.config
    }

    /// Returns whether streaming is enabled.
    pub fn is_streaming(&self) -> bool {
        self.streaming
    }

    /// Returns the output strategy.
    pub fn output_strategy(&self) -> &OutputStrategy {
        &self.output_strategy
    }

    /// Returns the retry configuration, if any.
    pub fn retry(&self) -> Option<&RetryConfig> {
        self.retry.as_ref()
    }

    /// Returns the per-request timeout override, if any.
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    /// Set a system prompt template (enables `/api/chat` mode on Ollama).
    pub fn with_system(mut self, template: impl Into<String>) -> Self {
        self.system_template = Some(template.into());
        self
    }

    /// Set the model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Set the LLM configuration.
    pub fn with_config(mut self, config: LlmConfig) -> Self {
        self.config = config;
        self
    }

    /// Enable or disable streaming.
    pub fn with_streaming(mut self, enabled: bool) -> Self {
        self.streaming = enabled;
        self
    }

    /// Set a custom output strategy.
    pub fn with_output_strategy(mut self, strategy: OutputStrategy) -> Self {
        self.output_strategy = strategy;
        self
    }

    /// Set retry configuration.
    pub fn with_retry(mut self, retry: RetryConfig) -> Self {
        self.retry = Some(retry);
        self
    }

    /// Set a per-request timeout for this payload's HTTP calls.
    ///
    /// When set, each HTTP request to the LLM provider uses this timeout
    /// instead of the default [`PipelineLimits::request_timeout`](crate::limits::PipelineLimits::request_timeout).
    /// This allows mixed-latency payloads (e.g., a 5 s classifier and a
    /// 120 s generator) to coexist on the same [`ExecCtx`].
    ///
    /// # Example
    ///
    /// ```ignore
    /// use std::time::Duration;
    /// use llm_pipeline::LlmCall;
    ///
    /// let fast_call = LlmCall::new("classify", "Classify: {input}")
    ///     .with_timeout(Duration::from_secs(5));
    ///
    /// let slow_call = LlmCall::new("generate", "Write a story about: {input}")
    ///     .with_timeout(Duration::from_secs(120));
    /// ```
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Shorthand: expect JSON output (full multi-strategy extraction with repair).
    pub fn expecting_json(mut self) -> Self {
        self.output_strategy = OutputStrategy::Json;
        self
    }

    /// Shorthand: expect a string list.
    pub fn expecting_list(mut self) -> Self {
        self.output_strategy = OutputStrategy::StringList;
        self
    }

    /// Shorthand: expect one of the given choices.
    pub fn expecting_choice(mut self, choices: Vec<String>) -> Self {
        self.output_strategy = OutputStrategy::Choice(choices);
        self
    }

    /// Shorthand: expect a number.
    pub fn expecting_number(mut self) -> Self {
        self.output_strategy = OutputStrategy::Number;
        self
    }

    /// Shorthand: expect a number in a range.
    pub fn expecting_number_in_range(mut self, min: f64, max: f64) -> Self {
        self.output_strategy = OutputStrategy::NumberInRange(min, max);
        self
    }

    /// Shorthand: expect clean text output.
    pub fn expecting_text(mut self) -> Self {
        self.output_strategy = OutputStrategy::Text;
        self
    }

    /// Create from an existing [`Stage`](crate::stage::Stage) (for Pipeline compatibility).
    pub(crate) fn from_stage(stage: &crate::stage::Stage, streaming: bool) -> Self {
        Self {
            name: stage.name.clone(),
            prompt_template: stage.prompt_template.clone(),
            system_template: stage.system_prompt.clone(),
            model: stage.model.clone(),
            config: stage.config.clone(),
            streaming,
            output_strategy: OutputStrategy::default(),
            retry: None,
            timeout: None,
        }
    }

    /// Render the prompt template, substituting `{input}` and context vars.
    fn render_prompt(template: &str, input: &str, vars: &HashMap<String, String>) -> String {
        let mut rendered = template.replace("{input}", input);
        for (key, value) in vars {
            let placeholder = format!("{{{}}}", key);
            rendered = rendered.replace(&placeholder, value);
        }
        rendered
    }

    /// Render a template with context vars only (no {input}).
    fn render_system(template: &str, vars: &HashMap<String, String>) -> String {
        let mut rendered = template.to_string();
        for (key, value) in vars {
            let placeholder = format!("{{{}}}", key);
            rendered = rendered.replace(&placeholder, value);
        }
        rendered
    }

    /// Convert a `Value` input to a string for template substitution.
    fn input_to_string(input: &Value) -> String {
        match input {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }

    /// Build an `LlmRequest` from the current state.
    ///
    /// `effective_timeout` is the resolved per-request timeout: either the
    /// payload-specific override (`self.timeout`) or the context default
    /// (`ctx.limits.request_timeout`).
    fn build_request(
        &self,
        prompt: &str,
        system: Option<&str>,
        messages: Vec<ChatMessage>,
        stream: bool,
        effective_timeout: Duration,
    ) -> LlmRequest {
        LlmRequest {
            model: self.model.clone(),
            system_prompt: system.map(|s| s.to_string()),
            prompt: prompt.to_string(),
            messages,
            config: self.config.clone(),
            stream,
            request_timeout: Some(effective_timeout),
        }
    }

    /// Resolve the effective per-request timeout: payload override wins,
    /// otherwise falls back to context limits.
    fn effective_timeout(&self, ctx: &ExecCtx) -> Duration {
        self.timeout.unwrap_or(ctx.limits.request_timeout)
    }

    fn parser_options(ctx: &ExecCtx) -> ParseOptions {
        ParseOptions {
            max_input_bytes: ctx
                .limits
                .max_response_bytes
                .min(ParseOptions::default().max_input_bytes),
            ..ParseOptions::default()
        }
    }

    fn enforce_response_size(ctx: &ExecCtx, size: usize) -> Result<()> {
        if size > ctx.limits.max_response_bytes {
            return Err(crate::PipelineError::ResponseTooLarge {
                size,
                limit: ctx.limits.max_response_bytes,
            });
        }

        Ok(())
    }

    async fn wait_for_stream_idle(
        idle_timeout: std::time::Duration,
        mut rx: watch::Receiver<StdInstant>,
    ) {
        loop {
            let deadline = *rx.borrow() + idle_timeout;
            let sleep = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
            tokio::pin!(sleep);

            tokio::select! {
                _ = &mut sleep => return,
                changed = rx.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
            }
        }
    }

    /// Execute via the backend (non-streaming), tracking transport retries.
    ///
    /// Returns `(LlmResponse, transport_retries, backoff_total_ms)`.
    async fn call_backend(
        &self,
        ctx: &ExecCtx,
        request: &LlmRequest,
    ) -> Result<(LlmResponse, u32, u64)> {
        let mut transport_retries: u32 = 0;
        let mut backoff_total_ms: u64 = 0;
        let name = self.name.clone();
        let event_handler = ctx.event_handler.clone();

        let mut on_retry = |attempt: u32, delay: std::time::Duration, reason: &str| {
            transport_retries = attempt;
            backoff_total_ms += delay.as_millis() as u64;
            emit(
                &event_handler,
                Event::TransportRetry {
                    name: name.clone(),
                    attempt,
                    delay_ms: delay.as_millis() as u64,
                    reason: reason.to_string(),
                },
            );
        };

        let response = backend::with_backoff(
            &ctx.backend,
            &ctx.client,
            &ctx.base_url,
            request,
            &ctx.backoff,
            ctx.cancel_flag(),
            Some(&mut on_retry),
        )
        .await?;

        Ok((response, transport_retries, backoff_total_ms))
    }

    /// Execute via the backend (streaming), emitting Token events and tracking transport retries.
    ///
    /// Returns `(LlmResponse, transport_retries, backoff_total_ms)`.
    async fn call_backend_streaming(
        &self,
        ctx: &ExecCtx,
        request: &LlmRequest,
    ) -> Result<(LlmResponse, u32, u64)> {
        let retry_stats = Arc::new(Mutex::new((0u32, 0u64)));
        let retry_name = self.name.clone();
        let retry_event_handler = ctx.event_handler.clone();

        let retry_stats_for_cb = Arc::clone(&retry_stats);
        let mut on_retry = |attempt: u32, delay: std::time::Duration, reason: &str| {
            if let Ok(mut stats) = retry_stats_for_cb.lock() {
                stats.0 = attempt;
                stats.1 += delay.as_millis() as u64;
            }
            emit(
                &retry_event_handler,
                Event::TransportRetry {
                    name: retry_name.clone(),
                    attempt,
                    delay_ms: delay.as_millis() as u64,
                    reason: reason.to_string(),
                },
            );
        };

        let name = self.name.clone();
        let event_handler = ctx.event_handler.clone();
        let (idle_tx, idle_rx) = watch::channel(StdInstant::now());
        let (limit_tx, mut limit_rx) = mpsc::unbounded_channel();
        let max_response_bytes = ctx.limits.max_response_bytes;
        let mut streamed_bytes = 0usize;

        let mut on_token = move |token: String| {
            streamed_bytes += token.len();
            let _ = idle_tx.send(StdInstant::now());

            if streamed_bytes > max_response_bytes {
                let _ = limit_tx.send(streamed_bytes);
                return;
            }

            emit(
                &event_handler,
                Event::Token {
                    name: name.clone(),
                    chunk: token,
                },
            );
        };

        let idle_timeout = ctx.limits.stream_idle_timeout;
        let backend_call = backend::with_backoff_streaming(
            &ctx.backend,
            &ctx.client,
            &ctx.base_url,
            request,
            &ctx.backoff,
            backend::BackoffStreamOpts {
                cancel: ctx.cancel_flag(),
                on_retry: Some(&mut on_retry),
                on_token: &mut on_token,
            },
        );
        tokio::pin!(backend_call);

        let idle_watch = Self::wait_for_stream_idle(idle_timeout, idle_rx);
        tokio::pin!(idle_watch);

        let response = tokio::select! {
            response = &mut backend_call => response?,
            Some(size) = limit_rx.recv() => {
                return Err(crate::PipelineError::ResponseTooLarge {
                    size,
                    limit: max_response_bytes,
                });
            }
            _ = &mut idle_watch => {
                return Err(crate::PipelineError::StreamIdle {
                    idle_ms: idle_timeout.as_millis() as u64,
                    limit_ms: idle_timeout.as_millis() as u64,
                });
            }
        };

        let (transport_retries, backoff_total_ms) =
            retry_stats.lock().map(|stats| *stats).unwrap_or((0, 0));

        Ok((response, transport_retries, backoff_total_ms))
    }

    /// Check if a retry is needed. Returns `Some(reason)` if retry needed, `None` if output is ok.
    fn check_retry_needed(
        &self,
        output: &PayloadOutput,
        retry_config: &RetryConfig,
    ) -> Option<String> {
        // Check parse error from OutputStrategy
        if let Some(ref diag) = output.diagnostics {
            if let Some(ref err) = diag.parse_error {
                return Some(err.clone());
            }
        }

        // Check semantic validator
        if let Some(ref validator) = retry_config.validator {
            if let Err(reason) = validator(&output.raw_response, &output.value) {
                return Some(reason);
            }
        }

        None
    }

    /// Build a `PayloadOutput` from raw LLM text using the configured `OutputStrategy`.
    ///
    /// Per CLAUDE.md: `build_output` MUST always return `Ok(PayloadOutput)`.
    /// Parse failures go into `diagnostics.parse_error`, not `Err`.
    fn build_output(&self, raw_text: String, parser_opts: &ParseOptions) -> PayloadOutput {
        let response_bytes = raw_text.len();
        let (thinking, cleaned) = parsing::extract_thinking(&raw_text);

        let mut diag = ParseDiagnostics::default();

        let value = match &self.output_strategy {
            OutputStrategy::Lossy => {
                diag.strategy = Some("lossy");
                parsing::parse_value_lossy(&cleaned)
            }
            OutputStrategy::Json => {
                diag.strategy = Some("json");
                match output_parser::parse_json_value_with_trace(&cleaned, parser_opts) {
                    Ok((v, trace)) => {
                        diag.apply_trace(trace);
                        v
                    }
                    Err(e) => {
                        diag.parse_error = Some(e.to_string());
                        // Fallback: try lossy parse
                        parsing::parse_value_lossy(&cleaned)
                    }
                }
            }
            OutputStrategy::StringList => {
                diag.strategy = Some("string_list");
                match output_parser::parse_string_list_with_trace(&cleaned, parser_opts) {
                    Ok((items, trace)) => {
                        diag.apply_trace(trace);
                        Value::Array(items.into_iter().map(Value::String).collect())
                    }
                    Err(e) => {
                        diag.parse_error = Some(e.to_string());
                        Value::String(cleaned.clone())
                    }
                }
            }
            OutputStrategy::XmlTag(tag) => {
                diag.strategy = Some("xml_tag");
                match output_parser::parse_xml_tag_with_trace(&cleaned, tag, parser_opts) {
                    Ok((content, trace)) => {
                        diag.apply_trace(trace);
                        Value::String(content)
                    }
                    Err(e) => {
                        diag.parse_error = Some(e.to_string());
                        Value::String(cleaned.clone())
                    }
                }
            }
            OutputStrategy::Choice(choices) => {
                diag.strategy = Some("choice");
                let choice_refs: Vec<&str> = choices.iter().map(|s| s.as_str()).collect();
                match output_parser::parse_choice_with_trace(&cleaned, &choice_refs, parser_opts) {
                    Ok((matched, trace)) => {
                        diag.apply_trace(trace);
                        Value::String(matched.to_string())
                    }
                    Err(e) => {
                        diag.parse_error = Some(e.to_string());
                        Value::String(cleaned.clone())
                    }
                }
            }
            OutputStrategy::Number => {
                diag.strategy = Some("number");
                match output_parser::parse_number_with_trace::<f64>(&cleaned, parser_opts) {
                    Ok((n, trace)) => {
                        diag.apply_trace(trace);
                        json!(n)
                    }
                    Err(e) => {
                        diag.parse_error = Some(e.to_string());
                        Value::String(cleaned.clone())
                    }
                }
            }
            OutputStrategy::NumberInRange(min, max) => {
                diag.strategy = Some("number_in_range");
                match output_parser::parse_number_in_range_with_trace::<f64>(
                    &cleaned,
                    *min,
                    *max,
                    parser_opts,
                ) {
                    Ok((n, trace)) => {
                        diag.apply_trace(trace);
                        json!(n)
                    }
                    Err(e) => {
                        diag.parse_error = Some(e.to_string());
                        Value::String(cleaned.clone())
                    }
                }
            }
            OutputStrategy::Text => {
                diag.strategy = Some("text");
                match output_parser::parse_text_with_trace(&cleaned, parser_opts) {
                    Ok((text, trace)) => {
                        diag.apply_trace(trace);
                        Value::String(text)
                    }
                    Err(e) => {
                        diag.parse_error = Some(e.to_string());
                        Value::String(cleaned.clone())
                    }
                }
            }
            OutputStrategy::Custom(f) => {
                diag.strategy = Some("custom");
                match f(&cleaned) {
                    Ok(v) => v,
                    Err(e) => {
                        diag.parse_error = Some(e.to_string());
                        Value::String(cleaned.clone())
                    }
                }
            }
        };

        PayloadOutput {
            value,
            raw_response: raw_text,
            thinking,
            model: Some(self.model.clone()),
            diagnostics: Some(diag),
            trace_id: None,  // Set by invoke()
            trace_ctx: None, // Set by invoke()
            transport_retries_used: 0,
            semantic_retries_used: 0,
            response_bytes,
            wall_time_ms: 0,
        }
    }
}

#[allow(deprecated)]
impl Payload for LlmCall {
    fn kind(&self) -> &'static str {
        "llm-call"
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn invoke<'a>(&'a self, ctx: &'a ExecCtx, input: Value) -> BoxFut<'a, Result<PayloadOutput>> {
        Box::pin(async move {
            let start = std::time::Instant::now();
            ctx.check_cancelled()?;

            emit(
                &ctx.event_handler,
                Event::PayloadStart {
                    name: self.name.clone(),
                    kind: self.kind(),
                },
            );

            let execution = async {
                let parser_opts = Self::parser_options(ctx);
                let input_str = Self::input_to_string(&input);
                let prompt = Self::render_prompt(&self.prompt_template, &input_str, &ctx.vars);
                let system = self
                    .system_template
                    .as_ref()
                    .map(|t| Self::render_system(t, &ctx.vars));

                let effective_timeout = self.effective_timeout(ctx);
                let request = self.build_request(
                    &prompt,
                    system.as_deref(),
                    Vec::new(),
                    self.streaming,
                    effective_timeout,
                );

                let result = if self.streaming {
                    self.call_backend_streaming(ctx, &request).await
                } else {
                    self.call_backend(ctx, &request).await
                };

                let (response, mut total_transport_retries, mut total_backoff_total_ms) = result?;
                Self::enforce_response_size(ctx, response.text.len())?;

                let mut semantic_retries_used = 0u32;
                let mut output = self.build_output(response.text, &parser_opts);
                if let Some(ref mut diag) = output.diagnostics {
                    diag.transport_retries = total_transport_retries;
                    diag.backoff_total_ms = total_backoff_total_ms;
                }

                // Structured retry identifiers: one AttemptId per logical retry
                // family, one TrialId per concrete execution within that family.
                let mut retry_attempt_id: Option<AttemptId> = None;
                let mut retry_trial_id: Option<TrialId> = None;

                if let Some(ref retry_config) = self.retry {
                    let mut retry_reason = self.check_retry_needed(&output, retry_config);

                    if retry_reason.is_some() {
                        // Create the AttemptId once for this retry family
                        let attempt_id = AttemptId::generate();
                        retry_attempt_id = Some(attempt_id.clone());

                        let mut messages = vec![ChatMessage {
                            role: backend::Role::User,
                            content: prompt.clone(),
                        }];
                        let mut temp_offset = 0.0f64;

                        for attempt in 1..=retry_config.max_retries {
                            ctx.check_cancelled()?;

                            let reason = retry_reason.take().unwrap_or_default();

                            // Each retry attempt gets a new TrialId
                            let trial_id = TrialId::generate();
                            retry_trial_id = Some(trial_id.clone());

                            emit(
                                &ctx.event_handler,
                                Event::RetryStart {
                                    name: self.name.clone(),
                                    attempt,
                                    reason: reason.clone(),
                                    attempt_id: attempt_id.clone(),
                                    trial_id: trial_id.clone(),
                                },
                            );

                            messages.push(ChatMessage {
                                role: backend::Role::Assistant,
                                content: output.raw_response.clone(),
                            });
                            messages.push(ChatMessage {
                                role: backend::Role::User,
                                content: format!(
                                    "Your previous response was invalid: {}. Please try again with the correct format.",
                                    reason
                                ),
                            });

                            if retry_config.cool_down {
                                temp_offset += 0.2;
                            }

                            let mut retry_config_clone = self.config.clone();
                            retry_config_clone.temperature =
                                (retry_config_clone.temperature - temp_offset).max(0.0);

                            let retry_request = LlmRequest {
                                model: self.model.clone(),
                                system_prompt: system.clone(),
                                prompt: prompt.clone(),
                                messages: messages.clone(),
                                config: retry_config_clone,
                                stream: false,
                                request_timeout: Some(effective_timeout),
                            };

                            let (retry_response, tr, bt) =
                                self.call_backend(ctx, &retry_request).await?;
                            total_transport_retries += tr;
                            total_backoff_total_ms += bt;
                            Self::enforce_response_size(ctx, retry_response.text.len())?;

                            semantic_retries_used = attempt;
                            output = self.build_output(retry_response.text, &parser_opts);

                            if let Some(ref mut diag) = output.diagnostics {
                                diag.retry_attempts = semantic_retries_used;
                                diag.transport_retries = total_transport_retries;
                                diag.backoff_total_ms = total_backoff_total_ms;
                                diag.attempt_id = Some(attempt_id.clone());
                                diag.trial_id = Some(trial_id);
                            }

                            retry_reason = self.check_retry_needed(&output, retry_config);

                            emit(
                                &ctx.event_handler,
                                Event::RetryEnd {
                                    name: self.name.clone(),
                                    attempts: attempt,
                                    success: retry_reason.is_none(),
                                    attempt_id: attempt_id.clone(),
                                },
                            );

                            if retry_reason.is_none() {
                                break;
                            }
                        }
                    }
                }

                if let Some(ref mut diag) = output.diagnostics {
                    diag.retry_attempts = semantic_retries_used;
                    diag.transport_retries = total_transport_retries;
                    diag.backoff_total_ms = total_backoff_total_ms;
                    // Persist final retry identifiers on diagnostics
                    if retry_attempt_id.is_some() {
                        diag.attempt_id = retry_attempt_id;
                        diag.trial_id = retry_trial_id;
                    }
                }

                output.trace_id = Some(ctx.trace_id.clone());
                output.trace_ctx = Some(ctx.trace_ctx.clone());
                output.transport_retries_used = total_transport_retries;
                output.semantic_retries_used = semantic_retries_used;
                output.wall_time_ms = start.elapsed().as_millis() as u64;

                Ok(output)
            };

            let result = tokio::time::timeout(ctx.limits.request_timeout, execution).await;
            match result {
                Ok(Ok(output)) => {
                    emit(
                        &ctx.event_handler,
                        Event::PayloadEnd {
                            name: self.name.clone(),
                            ok: true,
                        },
                    );
                    Ok(output)
                }
                Ok(Err(err)) => {
                    emit(
                        &ctx.event_handler,
                        Event::PayloadEnd {
                            name: self.name.clone(),
                            ok: false,
                        },
                    );
                    Err(err)
                }
                Err(_) => {
                    emit(
                        &ctx.event_handler,
                        Event::PayloadEnd {
                            name: self.name.clone(),
                            ok: false,
                        },
                    );
                    Err(crate::PipelineError::Timeout {
                        elapsed_ms: ctx.limits.request_timeout.as_millis() as u64,
                        limit_ms: ctx.limits.request_timeout.as_millis() as u64,
                    })
                }
            }
        })
    }
}

#[allow(deprecated)]
#[cfg(test)]
#[path = "llm_call_tests.rs"]
mod tests;
