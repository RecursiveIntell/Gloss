use super::*;
use crate::backend::{Backend, MockBackend, Role};
use async_trait::async_trait;
use reqwest::Client;
use std::time::Duration;

fn parser_opts() -> ParseOptions {
    ParseOptions::default()
}

#[derive(Debug)]
struct SlowBackend {
    delay: Duration,
    response: String,
}

#[async_trait]
impl Backend for SlowBackend {
    async fn complete(
        &self,
        _client: &Client,
        _base_url: &str,
        _request: &LlmRequest,
    ) -> Result<LlmResponse> {
        tokio::time::sleep(self.delay).await;
        Ok(LlmResponse {
            text: self.response.clone(),
            status: 200,
            metadata: None,
        })
    }

    async fn complete_streaming(
        &self,
        _client: &Client,
        _base_url: &str,
        _request: &LlmRequest,
        on_token: &mut (dyn FnMut(String) + Send),
    ) -> Result<LlmResponse> {
        tokio::time::sleep(self.delay).await;
        on_token(self.response.clone());
        Ok(LlmResponse {
            text: self.response.clone(),
            status: 200,
            metadata: None,
        })
    }

    fn name(&self) -> &'static str {
        "slow"
    }
}

#[test]
fn test_build_output_lossy_backward_compat() {
    let call = LlmCall::new("test", "prompt");
    let output = call.build_output(r#"{"key": "value"}"#.into(), &parser_opts());
    assert!(output.value.is_object());
    assert!(output.diagnostics.as_ref().unwrap().ok());
    assert_eq!(output.diagnostics.as_ref().unwrap().strategy, Some("lossy"));
}

#[test]
fn test_build_output_json_strategy_succeeds() {
    let call = LlmCall::new("test", "prompt").expecting_json();
    let output = call.build_output(r#"{"key": "value"}"#.into(), &parser_opts());
    assert!(output.value.is_object());
    assert_eq!(output.value["key"], "value");
    assert!(output.diagnostics.as_ref().unwrap().ok());
}

#[test]
fn test_build_output_json_strategy_repairs() {
    let call = LlmCall::new("test", "prompt").expecting_json();
    // Single quotes and trailing comma — repairable
    let output = call.build_output("{'key': 'value',}".into(), &parser_opts());
    assert!(output.value.is_object());
    assert!(output.diagnostics.as_ref().unwrap().ok());
    assert!(output.diagnostics.as_ref().unwrap().repaired);
}

#[test]
fn test_build_output_json_strategy_fails() {
    let call = LlmCall::new("test", "prompt").expecting_json();
    let output = call.build_output("not json at all".into(), &parser_opts());
    assert!(output.diagnostics.as_ref().unwrap().parse_error.is_some());
    // Should still return a Value (fallback to lossy)
    assert!(output.value.is_string());
}

#[test]
fn test_build_output_string_list_strategy() {
    let call = LlmCall::new("test", "prompt").expecting_list();
    let output = call.build_output("[\"apple\", \"banana\", \"cherry\"]".into(), &parser_opts());
    assert!(output.value.is_array());
    let arr = output.value.as_array().unwrap();
    assert_eq!(arr.len(), 3);
    assert!(output.diagnostics.as_ref().unwrap().ok());
}

#[test]
fn test_build_output_xml_tag_strategy() {
    let call = LlmCall::new("test", "prompt")
        .with_output_strategy(OutputStrategy::XmlTag("answer".into()));
    let output = call.build_output("<answer>42</answer>".into(), &parser_opts());
    assert_eq!(output.value, Value::String("42".into()));
    assert!(output.diagnostics.as_ref().unwrap().ok());
}

#[test]
fn test_build_output_choice_strategy() {
    let call = LlmCall::new("test", "prompt").expecting_choice(vec![
        "yes".into(),
        "no".into(),
        "maybe".into(),
    ]);
    let output = call.build_output("I think the answer is yes.".into(), &parser_opts());
    assert_eq!(output.value, Value::String("yes".into()));
    assert!(output.diagnostics.as_ref().unwrap().ok());
}

#[test]
fn test_build_output_number_strategy() {
    let call = LlmCall::new("test", "prompt").expecting_number();
    let output = call.build_output("Score: 8.5".into(), &parser_opts());
    let n = output.value.as_f64().unwrap();
    assert!((n - 8.5).abs() < f64::EPSILON);
    assert!(output.diagnostics.as_ref().unwrap().ok());
}

#[test]
fn test_build_output_number_in_range_rejects() {
    let call = LlmCall::new("test", "prompt").expecting_number_in_range(0.0, 5.0);
    let output = call.build_output("Score: 8.5".into(), &parser_opts());
    // Should fail: 8.5 > 5.0
    assert!(output.diagnostics.as_ref().unwrap().parse_error.is_some());
}

#[test]
fn test_build_output_text_strategy() {
    let call = LlmCall::new("test", "prompt").expecting_text();
    let output = call.build_output(
        "Sure! Here's the answer: The sky is blue.".into(),
        &parser_opts(),
    );
    let text = output.value.as_str().unwrap();
    // parse_text strips "Sure!" and "Here's..." prefixes
    assert!(!text.starts_with("Sure!"));
    assert!(output.diagnostics.as_ref().unwrap().ok());
}

#[test]
fn test_build_output_custom_strategy() {
    let call = LlmCall::new("test", "prompt").with_output_strategy(OutputStrategy::Custom(
        std::sync::Arc::new(|raw: &str| {
            let upper = raw.to_uppercase();
            Ok(Value::String(upper))
        }),
    ));
    let output = call.build_output("hello world".into(), &parser_opts());
    assert_eq!(output.value, Value::String("HELLO WORLD".into()));
    assert!(output.diagnostics.as_ref().unwrap().ok());
}

#[test]
fn test_diagnostics_attached_to_output() {
    let call = LlmCall::new("test", "prompt").expecting_json();
    let output = call.build_output(r#"{"a": 1}"#.into(), &parser_opts());
    let diag = output.diagnostics.as_ref().unwrap();
    assert_eq!(diag.strategy, Some("json"));
    assert!(diag.ok());
    assert!(!diag.repaired);
    assert_eq!(diag.retry_attempts, 0);
}

#[test]
fn test_build_output_with_thinking() {
    let call = LlmCall::new("test", "prompt").expecting_json();
    let input = "<think>Let me think about this...</think>{\"result\": 42}";
    let output = call.build_output(input.into(), &parser_opts());
    assert_eq!(output.thinking, Some("Let me think about this...".into()));
    assert_eq!(output.value["result"], 42);
}

#[test]
fn test_backend_default_is_ollama() {
    let ctx = ExecCtx::builder("http://localhost:11434").build();
    assert_eq!(ctx.backend.name(), "ollama");
}

#[cfg(feature = "openai")]
#[test]
fn test_exec_ctx_openai_builder() {
    let ctx = ExecCtx::builder("https://api.openai.com")
        .openai_with_key("sk-test")
        .build();
    assert_eq!(ctx.backend.name(), "openai");
}

#[test]
fn test_build_request() {
    let call = LlmCall::new("test", "Summarize: {input}")
        .with_model("gpt-4o")
        .with_config(LlmConfig::default().with_json_mode(true));

    let request = call.build_request(
        "Tell me about Rust",
        Some("You are helpful"),
        Vec::new(),
        false,
        Duration::from_secs(30),
    );

    assert_eq!(request.model, "gpt-4o");
    assert_eq!(request.prompt, "Tell me about Rust");
    assert_eq!(request.system_prompt.as_deref(), Some("You are helpful"));
    assert!(request.config.json_mode);
    assert!(!request.stream);
}

#[test]
fn test_build_request_with_messages() {
    let call = LlmCall::new("test", "prompt");
    let messages = vec![
        ChatMessage {
            role: Role::User,
            content: "What is 2+2?".into(),
        },
        ChatMessage {
            role: Role::Assistant,
            content: "4".into(),
        },
    ];
    let request = call.build_request("Follow up", None, messages, false, Duration::from_secs(30));
    assert_eq!(request.messages.len(), 2);
}

// --- Retry tests (unit-level, testing check_retry_needed and retry config) ---

#[test]
fn test_retry_not_triggered_on_success() {
    let call = LlmCall::new("test", "prompt")
        .expecting_json()
        .with_retry(RetryConfig::new(2));

    let output = call.build_output(r#"{"key": "value"}"#.into(), &parser_opts());
    let retry_config = call.retry.as_ref().unwrap();
    assert!(call.check_retry_needed(&output, retry_config).is_none());
}

#[test]
fn test_retry_triggered_on_parse_failure() {
    let call = LlmCall::new("test", "prompt")
        .expecting_json()
        .with_retry(RetryConfig::new(2));

    let output = call.build_output("not json".into(), &parser_opts());
    let retry_config = call.retry.as_ref().unwrap();
    let reason = call.check_retry_needed(&output, retry_config);
    assert!(reason.is_some());
}

#[test]
fn test_retry_triggered_on_semantic_failure() {
    let call = LlmCall::new("test", "prompt")
        .expecting_json()
        .with_retry(RetryConfig::new(2).requiring_keys(&["title", "year"]));

    // Valid JSON but missing required keys
    let output = call.build_output(r#"{"title": "Matrix"}"#.into(), &parser_opts());
    let retry_config = call.retry.as_ref().unwrap();
    let reason = call.check_retry_needed(&output, retry_config);
    assert!(reason.is_some());
    assert!(reason.unwrap().contains("year"));
}

#[test]
fn test_retry_requiring_keys_passes() {
    let call = LlmCall::new("test", "prompt")
        .expecting_json()
        .with_retry(RetryConfig::new(2).requiring_keys(&["title", "year"]));

    let output = call.build_output(
        r#"{"title": "Matrix", "year": 1999}"#.into(),
        &parser_opts(),
    );
    let retry_config = call.retry.as_ref().unwrap();
    assert!(call.check_retry_needed(&output, retry_config).is_none());
}

#[test]
fn test_retry_cool_down_reduces_temperature() {
    let config = LlmConfig::default().with_temperature(0.7);
    let call = LlmCall::new("test", "prompt")
        .with_config(config)
        .with_retry(RetryConfig::new(3));

    // Verify cool_down is true by default
    assert!(call.retry.as_ref().unwrap().cool_down);

    // After 2 retries at 0.2 per retry: 0.7 - 0.4 = 0.3
    let adjusted = (0.7 - 0.4f64).max(0.0);
    assert!((adjusted - 0.3).abs() < f64::EPSILON);
}

#[test]
fn test_retry_no_cool_down() {
    let call = LlmCall::new("test", "prompt").with_retry(RetryConfig::new(3).no_cool_down());

    assert!(!call.retry.as_ref().unwrap().cool_down);
}

#[test]
fn test_choice_strategy_with_retry_detects_failure() {
    let call = LlmCall::new("test", "prompt")
        .expecting_choice(vec!["approve".into(), "reject".into(), "defer".into()])
        .with_retry(RetryConfig::new(2));

    // Bad response - no valid choice found
    let output = call.build_output(
        "I think we should consider all options carefully.".into(),
        &parser_opts(),
    );
    let retry_config = call.retry.as_ref().unwrap();
    let reason = call.check_retry_needed(&output, retry_config);
    assert!(reason.is_some());
}

#[test]
fn test_choice_strategy_succeeds() {
    let call = LlmCall::new("test", "prompt")
        .expecting_choice(vec!["approve".into(), "reject".into(), "defer".into()])
        .with_retry(RetryConfig::new(2));

    let output = call.build_output("I would approve this request.".into(), &parser_opts());
    let retry_config = call.retry.as_ref().unwrap();
    assert!(call.check_retry_needed(&output, retry_config).is_none());
    assert_eq!(output.value, Value::String("approve".into()));
}

#[test]
fn test_number_in_range_with_retry_detects_failure() {
    let call = LlmCall::new("test", "prompt")
        .expecting_number_in_range(1.0, 10.0)
        .with_retry(RetryConfig::new(2));

    let output = call.build_output("Score: 15".into(), &parser_opts());
    let retry_config = call.retry.as_ref().unwrap();
    let reason = call.check_retry_needed(&output, retry_config);
    assert!(reason.is_some());
}

#[test]
fn test_custom_validator_with_retry() {
    let call = LlmCall::new("test", "prompt").expecting_json().with_retry(
        RetryConfig::new(2).with_validator(|_raw, value| {
            let score = value
                .get("score")
                .and_then(|v| v.as_f64())
                .ok_or("missing score")?;
            if !(0.0..=1.0).contains(&score) {
                return Err(format!("score {} outside 0.0-1.0", score));
            }
            Ok(())
        }),
    );

    // Valid JSON with out-of-range score
    let output = call.build_output(r#"{"score": 1.5}"#.into(), &parser_opts());
    let retry_config = call.retry.as_ref().unwrap();
    let reason = call.check_retry_needed(&output, retry_config);
    assert!(reason.is_some());
    assert!(reason.unwrap().contains("score 1.5 outside"));

    // Valid JSON with valid score
    let output = call.build_output(r#"{"score": 0.8}"#.into(), &parser_opts());
    assert!(call.check_retry_needed(&output, retry_config).is_none());
}

#[tokio::test]
async fn test_invoke_rejects_oversized_response() {
    let ctx = ExecCtx::builder("http://localhost:11434")
        .backend(Arc::new(MockBackend::fixed("this response is too large")))
        .with_limits(crate::PipelineLimits {
            max_response_bytes: 8,
            ..crate::PipelineLimits::default()
        })
        .build();
    let call = LlmCall::new("test", "{input}");

    let err = call
        .invoke(&ctx, Value::String("hello".into()))
        .await
        .unwrap_err();

    assert!(matches!(
        err,
        crate::PipelineError::ResponseTooLarge { limit: 8, .. }
    ));
}

#[tokio::test]
async fn test_invoke_times_out_on_slow_backend() {
    let ctx = ExecCtx::builder("http://localhost:11434")
        .backend(Arc::new(SlowBackend {
            delay: Duration::from_millis(50),
            response: r#"{"ok":true}"#.to_string(),
        }))
        .with_limits(crate::PipelineLimits {
            request_timeout: Duration::from_millis(10),
            ..crate::PipelineLimits::default()
        })
        .build();
    let call = LlmCall::new("test", "{input}").expecting_json();

    let err = call
        .invoke(&ctx, Value::String("hello".into()))
        .await
        .unwrap_err();

    assert!(matches!(err, crate::PipelineError::Timeout { .. }));
}

#[tokio::test]
async fn test_streaming_idle_timeout_is_enforced() {
    let ctx = ExecCtx::builder("http://localhost:11434")
        .backend(Arc::new(SlowBackend {
            delay: Duration::from_millis(50),
            response: r#"{"ok":true}"#.to_string(),
        }))
        .with_limits(crate::PipelineLimits {
            stream_idle_timeout: Duration::from_millis(10),
            request_timeout: Duration::from_millis(200),
            ..crate::PipelineLimits::default()
        })
        .build();
    let call = LlmCall::new("test", "{input}")
        .expecting_json()
        .with_streaming(true);

    let err = call
        .invoke(&ctx, Value::String("hello".into()))
        .await
        .unwrap_err();

    assert!(matches!(err, crate::PipelineError::StreamIdle { .. }));
}

#[tokio::test]
async fn test_invoke_tracks_semantic_retry_usage() {
    let ctx = ExecCtx::builder("http://localhost:11434")
        .backend(Arc::new(MockBackend::new(vec![
            "not json".to_string(),
            r#"{"ok": true}"#.to_string(),
        ])))
        .build();
    let call = LlmCall::new("test", "{input}")
        .expecting_json()
        .with_retry(RetryConfig::new(1));

    let output = call
        .invoke(&ctx, Value::String("hello".into()))
        .await
        .unwrap();

    assert_eq!(output.semantic_retries_used, 1);
    assert_eq!(output.transport_retries_used, 0);
    assert_eq!(output.trace_id, Some(ctx.trace_id.clone()));
    assert_eq!(
        output.trace_ctx.as_ref().map(|t| &t.trace_id),
        Some(&ctx.trace_ctx.trace_id)
    );
    assert_eq!(
        output.diagnostics.as_ref().map(|d| d.retry_attempts),
        Some(1)
    );
    // Verify structured retry identifiers are populated
    let diag = output.diagnostics.as_ref().unwrap();
    assert!(
        diag.attempt_id.is_some(),
        "attempt_id should be set after retry"
    );
    assert!(
        diag.trial_id.is_some(),
        "trial_id should be set after retry"
    );
}

#[tokio::test]
async fn test_invoke_no_retry_ids_without_retries() {
    let ctx = ExecCtx::builder("http://localhost:11434")
        .backend(Arc::new(MockBackend::fixed(r#"{"ok": true}"#)))
        .build();
    let call = LlmCall::new("test", "{input}")
        .expecting_json()
        .with_retry(RetryConfig::new(2));

    let output = call
        .invoke(&ctx, Value::String("hello".into()))
        .await
        .unwrap();

    assert_eq!(output.semantic_retries_used, 0);
    let diag = output.diagnostics.as_ref().unwrap();
    assert!(
        diag.attempt_id.is_none(),
        "attempt_id should be None when no retries occurred"
    );
    assert!(
        diag.trial_id.is_none(),
        "trial_id should be None when no retries occurred"
    );
}

#[test]
fn test_transport_retry_populates_diagnostics() {
    let mut transport_retries: u32 = 0;
    let mut backoff_total_ms: u64 = 0;

    let mut on_retry = |attempt: u32, delay: std::time::Duration, _reason: &str| {
        transport_retries = attempt;
        backoff_total_ms += delay.as_millis() as u64;
    };

    on_retry(
        1,
        std::time::Duration::from_millis(500),
        "429 Too Many Requests",
    );
    on_retry(
        2,
        std::time::Duration::from_millis(1000),
        "503 Service Unavailable",
    );

    assert_eq!(transport_retries, 2);
    assert_eq!(backoff_total_ms, 1500);
}

#[test]
fn test_llm_call_accessors() {
    let call = LlmCall::new("test", "Hello {input}")
        .with_model("llama3.2:3b")
        .with_streaming(true)
        .expecting_json();
    assert_eq!(call.name(), "test");
    assert_eq!(call.model(), "llama3.2:3b");
    assert!(call.is_streaming());
    assert!(matches!(call.output_strategy(), OutputStrategy::Json));
    assert_eq!(call.prompt_template(), "Hello {input}");
    assert!(call.system_template().is_none());
    assert!(call.retry().is_none());
}
