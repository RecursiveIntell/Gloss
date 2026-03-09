//! Backend trait and normalized request/response types.
//!
//! The [`Backend`] trait abstracts over LLM providers, translating between
//! normalized [`LlmRequest`]/[`LlmResponse`] types and provider-specific
//! HTTP APIs. Built-in implementations: [`OllamaBackend`], [`OpenAiBackend`].
//!
//! ## Architecture
//!
//! ```text
//! LlmCall ──► LlmRequest ──► Backend::complete() ──► LlmResponse
//!                                    │
//!                         ┌──────────┴──────────┐
//!                    OllamaBackend         OpenAiBackend
//!                   /api/generate          /v1/chat/completions
//!                   /api/chat              SSE streaming
//!                   NDJSON streaming
//! ```

pub mod backoff;
pub mod mock;
pub mod ollama;
#[cfg(feature = "openai")]
pub mod openai;
pub mod recording;
#[cfg(feature = "openai")]
pub mod sse;

pub use backoff::BackoffConfig;
pub use mock::MockBackend;
pub use ollama::OllamaBackend;
#[cfg(feature = "openai")]
pub use openai::OpenAiBackend;
pub use recording::RecordingBackend;

use crate::client::LlmConfig;
use crate::error::Result;
use crate::PipelineError;
use async_trait::async_trait;
use reqwest::Client;
use std::sync::Arc;

/// Type alias for the callback invoked before each transport retry.
///
/// Arguments: `(attempt_number, delay_before_retry, reason_for_retry)`.
pub type RetryCallback<'a> = Option<&'a mut (dyn FnMut(u32, std::time::Duration, &str) + Send)>;

/// A normalized LLM request — provider-agnostic.
///
/// [`LlmCall`](crate::llm_call::LlmCall) builds this from its config.
/// The [`Backend`] translates it into the provider-specific HTTP request.
#[derive(Debug, Clone)]
pub struct LlmRequest {
    /// Model identifier (e.g. `"llama3.2:3b"`, `"gpt-4o"`).
    pub model: String,

    /// If `Some`, this is a chat-style call with a system prompt.
    /// If `None`, this is a generate-style call (prompt only).
    pub system_prompt: Option<String>,

    /// The user prompt text.
    pub prompt: String,

    /// For retry: prior conversation history (original prompt + bad response + correction).
    /// Empty for initial calls.
    pub messages: Vec<ChatMessage>,

    /// LLM configuration (temperature, max_tokens, json_mode, etc.).
    pub config: LlmConfig,

    /// Whether to use the streaming endpoint.
    pub stream: bool,
}

/// A single message in a chat conversation.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    /// The role of the message author.
    pub role: Role,
    /// The message content.
    pub content: String,
}

/// The role of a chat message author.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// System instructions.
    System,
    /// User input.
    User,
    /// Assistant (model) response.
    Assistant,
}

/// A normalized LLM response.
#[derive(Debug)]
pub struct LlmResponse {
    /// The generated text content.
    pub text: String,

    /// HTTP status code (for diagnostics/logging).
    pub status: u16,

    /// Provider-specific metadata (token counts, timing, model info).
    /// Stored as raw JSON — each provider returns different fields.
    pub metadata: Option<serde_json::Value>,
}

/// Abstraction over LLM providers.
///
/// Implementors translate between the normalized [`LlmRequest`]/[`LlmResponse`]
/// and the provider's HTTP API. The trait handles two modes: non-streaming
/// completion and streaming completion with token callbacks.
///
/// Built-in implementations: [`OllamaBackend`], [`OpenAiBackend`].
///
/// # Object Safety
///
/// This trait is object-safe and designed to be used as `Arc<dyn Backend>`.
#[async_trait]
pub trait Backend: Send + Sync {
    /// Execute a non-streaming LLM call.
    async fn complete(
        &self,
        client: &Client,
        base_url: &str,
        request: &LlmRequest,
    ) -> Result<LlmResponse>;

    /// Execute a streaming LLM call.
    ///
    /// `on_token` is called for each token as it arrives. The final
    /// accumulated text is returned as an [`LlmResponse`].
    async fn complete_streaming(
        &self,
        client: &Client,
        base_url: &str,
        request: &LlmRequest,
        on_token: &mut (dyn FnMut(String) + Send),
    ) -> Result<LlmResponse>;

    /// Human-readable name for logging and diagnostics.
    fn name(&self) -> &'static str;
}

/// Check whether a [`PipelineError`] is retryable based on the backoff config.
///
/// Retryable conditions:
/// - [`PipelineError::HttpError`] with a status in `config.retryable_statuses`
/// - [`PipelineError::Request`] (connection/transport errors)
pub fn is_retryable(error: &PipelineError, config: &BackoffConfig) -> bool {
    match error {
        PipelineError::HttpError { status, .. } => config.retryable_statuses.contains(status),
        PipelineError::Request(_) => true,
        _ => false,
    }
}

/// Execute a backend call with transport-level retry and exponential backoff.
///
/// Wraps `Backend::complete()` or `Backend::complete_streaming()` with automatic
/// retry on transient failures (429, 5xx, connection errors). Uses the
/// [`BackoffConfig`] to determine delay strategy and retry count.
///
/// Returns the first successful response, or the last error if all retries
/// are exhausted.
///
/// # Arguments
///
/// * `backend` — The LLM backend to call
/// * `client` — HTTP client for making requests
/// * `base_url` — Base URL for the API
/// * `request` — The normalized LLM request
/// * `config` — Backoff configuration
/// * `cancel` — Optional cancellation flag
/// * `on_retry` — Optional callback invoked before each retry with (attempt, delay, reason)
pub async fn with_backoff(
    backend: &Arc<dyn Backend>,
    client: &Client,
    base_url: &str,
    request: &LlmRequest,
    config: &BackoffConfig,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    mut on_retry: RetryCallback<'_>,
) -> Result<LlmResponse> {
    let mut last_error: Option<PipelineError> = None;

    for attempt in 0..=config.max_retries {
        // Check cancellation
        if let Some(flag) = cancel {
            if flag.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(PipelineError::Cancelled);
            }
        }

        // Wait for backoff delay (not on first attempt)
        if attempt > 0 {
            let delay = if let Some(PipelineError::HttpError {
                retry_after: Some(ra),
                ..
            }) = &last_error
            {
                if config.respect_retry_after {
                    *ra
                } else {
                    config.delay_for_attempt(attempt - 1)
                }
            } else {
                config.delay_for_attempt(attempt - 1)
            };

            let reason = last_error
                .as_ref()
                .map(|e| e.to_string())
                .unwrap_or_default();

            if let Some(ref mut cb) = on_retry {
                cb(attempt, delay, &reason);
            }

            tokio::time::sleep(delay).await;

            // Check cancellation after sleep
            if let Some(flag) = cancel {
                if flag.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(PipelineError::Cancelled);
                }
            }
        }

        match backend.complete(client, base_url, request).await {
            Ok(response) => return Ok(response),
            Err(e) => {
                if attempt < config.max_retries && is_retryable(&e, config) {
                    last_error = Some(e);
                    continue;
                }
                return Err(e);
            }
        }
    }

    // Should not reach here, but just in case
    Err(last_error.unwrap_or(PipelineError::Other(
        "backoff loop exited unexpectedly".into(),
    )))
}

/// Options for [`with_backoff_streaming`] — bundles the optional/callback parameters.
pub struct BackoffStreamOpts<'a> {
    /// Optional cancellation flag.
    pub cancel: Option<&'a std::sync::atomic::AtomicBool>,
    /// Optional callback invoked before each retry.
    pub on_retry: RetryCallback<'a>,
    /// Token callback — receives each token as it arrives.
    pub on_token: &'a mut (dyn FnMut(String) + Send),
}

/// Execute a streaming backend call with transport-level retry.
///
/// Same as [`with_backoff`] but for streaming calls. Note: each retry
/// restarts the stream from scratch — partial tokens from failed attempts
/// are discarded.
pub async fn with_backoff_streaming(
    backend: &Arc<dyn Backend>,
    client: &Client,
    base_url: &str,
    request: &LlmRequest,
    config: &BackoffConfig,
    opts: BackoffStreamOpts<'_>,
) -> Result<LlmResponse> {
    let BackoffStreamOpts {
        cancel,
        mut on_retry,
        on_token,
    } = opts;
    let mut last_error: Option<PipelineError> = None;

    for attempt in 0..=config.max_retries {
        if let Some(flag) = cancel {
            if flag.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(PipelineError::Cancelled);
            }
        }

        if attempt > 0 {
            let delay = if let Some(PipelineError::HttpError {
                retry_after: Some(ra),
                ..
            }) = &last_error
            {
                if config.respect_retry_after {
                    *ra
                } else {
                    config.delay_for_attempt(attempt - 1)
                }
            } else {
                config.delay_for_attempt(attempt - 1)
            };

            let reason = last_error
                .as_ref()
                .map(|e| e.to_string())
                .unwrap_or_default();

            if let Some(ref mut cb) = on_retry {
                cb(attempt, delay, &reason);
            }

            tokio::time::sleep(delay).await;

            if let Some(flag) = cancel {
                if flag.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(PipelineError::Cancelled);
                }
            }
        }

        match backend
            .complete_streaming(client, base_url, request, on_token)
            .await
        {
            Ok(response) => return Ok(response),
            Err(e) => {
                if attempt < config.max_retries && is_retryable(&e, config) {
                    last_error = Some(e);
                    continue;
                }
                return Err(e);
            }
        }
    }

    Err(last_error.unwrap_or(PipelineError::Other(
        "backoff loop exited unexpectedly".into(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_is_retryable_429() {
        let config = BackoffConfig::standard();
        let err = PipelineError::HttpError {
            status: 429,
            body: "rate limited".into(),
            retry_after: None,
        };
        assert!(is_retryable(&err, &config));
    }

    #[test]
    fn test_is_retryable_503() {
        let config = BackoffConfig::standard();
        let err = PipelineError::HttpError {
            status: 503,
            body: "service unavailable".into(),
            retry_after: None,
        };
        assert!(is_retryable(&err, &config));
    }

    #[test]
    fn test_is_retryable_400_not_retried() {
        let config = BackoffConfig::standard();
        let err = PipelineError::HttpError {
            status: 400,
            body: "bad request".into(),
            retry_after: None,
        };
        assert!(!is_retryable(&err, &config));
    }

    #[test]
    fn test_is_retryable_other_error_not_retried() {
        let config = BackoffConfig::standard();
        let err = PipelineError::Other("some error".into());
        assert!(!is_retryable(&err, &config));
    }

    #[test]
    fn test_is_retryable_cancelled_not_retried() {
        let config = BackoffConfig::standard();
        let err = PipelineError::Cancelled;
        assert!(!is_retryable(&err, &config));
    }

    #[test]
    fn test_backoff_none_no_retry() {
        let config = BackoffConfig::none();
        assert_eq!(config.max_retries, 0);
        // Even retryable errors won't be retried with max_retries=0
    }

    #[tokio::test]
    async fn test_backoff_respects_cancellation() {
        use std::sync::atomic::AtomicBool;

        let cancel = AtomicBool::new(true);
        let backend: Arc<dyn Backend> = Arc::new(OllamaBackend);
        let client = Client::new();
        let request = LlmRequest {
            model: "test".into(),
            system_prompt: None,
            prompt: "test".into(),
            messages: Vec::new(),
            config: LlmConfig::default(),
            stream: false,
        };

        let result = with_backoff(
            &backend,
            &client,
            "http://localhost:99999",
            &request,
            &BackoffConfig::standard(),
            Some(&cancel),
            None,
        )
        .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PipelineError::Cancelled));
    }

    #[test]
    fn test_backoff_respects_retry_after_parsing() {
        let err = PipelineError::HttpError {
            status: 429,
            body: "rate limited".into(),
            retry_after: Some(Duration::from_secs(30)),
        };

        if let PipelineError::HttpError { retry_after, .. } = err {
            assert_eq!(retry_after, Some(Duration::from_secs(30)));
        }
    }
}
