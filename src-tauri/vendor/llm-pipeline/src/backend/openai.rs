//! Backend for OpenAI-compatible APIs.
//!
//! [`OpenAiBackend`] covers: OpenAI, Anthropic (compat layer), vLLM,
//! llama.cpp server, LM Studio, Together AI, Groq, Mistral, Fireworks,
//! and Ollama's `/v1/` endpoint.
//!
//! Endpoint: `/v1/chat/completions` (always chat mode).
//! Streaming: SSE with `data: {"choices": [{"delta": {"content": "token"}}]}`.

use super::sse::SseDecoder;
use super::{Backend, LlmRequest, LlmResponse, Role};
use crate::error::Result;
use crate::PipelineError;
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde_json::{json, Value};

/// Backend for any OpenAI-compatible API.
///
/// Covers: OpenAI, Anthropic (compat), vLLM, llama.cpp, LM Studio,
/// Together AI, Groq, Mistral, Fireworks, Ollama (`/v1/`), and more.
///
/// # Example
///
/// ```
/// use llm_pipeline::backend::OpenAiBackend;
///
/// let backend = OpenAiBackend::new();
/// let with_key = OpenAiBackend::new().with_api_key("sk-...");
/// ```
#[derive(Clone)]
pub struct OpenAiBackend {
    /// Optional API key. If set, sent as `Authorization: Bearer {key}`.
    pub(crate) api_key: Option<String>,
    /// Optional organization ID. If set, sent as `OpenAI-Organization: {org}`.
    pub(crate) organization: Option<String>,
}

impl std::fmt::Debug for OpenAiBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiBackend")
            .field(
                "api_key",
                &self.api_key.as_ref().map(|k| {
                    if k.len() > 6 {
                        format!("{}***", &k[..6])
                    } else {
                        "***".to_string()
                    }
                }),
            )
            .field("organization", &self.organization)
            .finish()
    }
}

impl OpenAiBackend {
    /// Create a new OpenAI-compatible backend without authentication.
    pub fn new() -> Self {
        Self {
            api_key: None,
            organization: None,
        }
    }

    /// Set the API key for authentication.
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Set the organization ID header.
    pub fn with_organization(mut self, org: impl Into<String>) -> Self {
        self.organization = Some(org.into());
        self
    }

    /// Returns `true` if an API key has been configured.
    pub fn has_api_key(&self) -> bool {
        self.api_key.is_some()
    }

    /// Returns `true` if an organization ID has been configured.
    pub fn has_organization(&self) -> bool {
        self.organization.is_some()
    }

    /// Build the messages array for the OpenAI request.
    fn build_messages(request: &LlmRequest) -> Vec<Value> {
        let mut messages = Vec::new();

        // System prompt
        if let Some(ref sys) = request.system_prompt {
            if !sys.is_empty() {
                messages.push(json!({"role": "system", "content": sys}));
            }
        }

        // Prior conversation history (for retry)
        for msg in &request.messages {
            let role = match msg.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            messages.push(json!({"role": role, "content": msg.content}));
        }

        // Current user prompt (only if no messages in history)
        if request.messages.is_empty() {
            messages.push(json!({"role": "user", "content": request.prompt}));
        }

        messages
    }

    /// Build the request body for `/v1/chat/completions`.
    fn build_body(request: &LlmRequest, stream: bool) -> Value {
        let mut body = json!({
            "model": request.model,
            "messages": Self::build_messages(request),
            "temperature": request.config.temperature,
            "max_tokens": request.config.max_tokens,
            "stream": stream,
        });

        if request.config.json_mode {
            body["response_format"] = json!({"type": "json_object"});
        }

        // Note: `thinking` / `extended_thinking` are skipped silently for OpenAI.
        // Custom options are also skipped — they're Ollama-specific.

        body
    }

    /// Parse a `Retry-After` header value as seconds.
    fn parse_retry_after(value: &str) -> Option<std::time::Duration> {
        if let Ok(secs) = value.trim().parse::<u64>() {
            return Some(std::time::Duration::from_secs(secs));
        }
        None
    }

    /// Build the reqwest request with appropriate headers.
    fn build_http_request(
        &self,
        client: &Client,
        url: &str,
        body: &Value,
    ) -> reqwest::RequestBuilder {
        let mut req = client.post(url).json(body);

        if let Some(ref key) = self.api_key {
            req = req.header("Authorization", format!("Bearer {}", key));
        }
        if let Some(ref org) = self.organization {
            req = req.header("OpenAI-Organization", org.as_str());
        }

        req
    }

    /// Extract metadata from an OpenAI response.
    fn extract_metadata(json_resp: &Value) -> Option<Value> {
        let mut meta = serde_json::Map::new();
        if let Some(v) = json_resp.get("usage") {
            meta.insert("usage".into(), v.clone());
        }
        if let Some(v) = json_resp.get("model") {
            meta.insert("model".into(), v.clone());
        }
        if let Some(v) = json_resp.get("id") {
            meta.insert("id".into(), v.clone());
        }
        if meta.is_empty() {
            None
        } else {
            Some(Value::Object(meta))
        }
    }
}

impl Default for OpenAiBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Backend for OpenAiBackend {
    async fn complete(
        &self,
        client: &Client,
        base_url: &str,
        request: &LlmRequest,
    ) -> Result<LlmResponse> {
        let base = base_url.trim_end_matches('/');
        let url = format!("{}/v1/chat/completions", base);
        let body = Self::build_body(request, false);

        let resp = self
            .build_http_request(client, &url, &body)
            .send()
            .await
            .map_err(|e| {
                PipelineError::Other(format!("Failed to connect to LLM at {}: {}", url, e))
            })?;

        let status = resp.status().as_u16();

        if !resp.status().is_success() {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(Self::parse_retry_after);
            let text = resp.text().await.unwrap_or_default();
            return Err(PipelineError::HttpError {
                status,
                body: text,
                retry_after,
            });
        }

        let json_resp: Value = resp.json().await?;

        let text = json_resp
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Ok(LlmResponse {
            text,
            status,
            metadata: Self::extract_metadata(&json_resp),
        })
    }

    async fn complete_streaming(
        &self,
        client: &Client,
        base_url: &str,
        request: &LlmRequest,
        on_token: &mut (dyn FnMut(String) + Send),
    ) -> Result<LlmResponse> {
        let base = base_url.trim_end_matches('/');
        let url = format!("{}/v1/chat/completions", base);
        let body = Self::build_body(request, true);

        let resp = self
            .build_http_request(client, &url, &body)
            .send()
            .await
            .map_err(|e| {
                PipelineError::Other(format!("Failed to connect to LLM at {}: {}", url, e))
            })?;

        let status = resp.status().as_u16();

        if !resp.status().is_success() {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(Self::parse_retry_after);
            let text = resp.text().await.unwrap_or_default();
            return Err(PipelineError::HttpError {
                status,
                body: text,
                retry_after,
            });
        }

        let mut stream = resp.bytes_stream();
        let mut decoder = SseDecoder::new();
        let mut accumulated = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(PipelineError::Request)?;
            for json_val in decoder.decode(&chunk) {
                if let Some(content) = json_val
                    .get("choices")
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("delta"))
                    .and_then(|d| d.get("content"))
                    .and_then(|v| v.as_str())
                {
                    if !content.is_empty() {
                        accumulated.push_str(content);
                        on_token(content.to_string());
                    }
                }
            }
        }

        // Flush remaining SSE buffer
        for json_val in decoder.flush() {
            if let Some(content) = json_val
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("delta"))
                .and_then(|d| d.get("content"))
                .and_then(|v| v.as_str())
            {
                if !content.is_empty() {
                    accumulated.push_str(content);
                    on_token(content.to_string());
                }
            }
        }

        Ok(LlmResponse {
            text: accumulated,
            status,
            metadata: None,
        })
    }

    fn name(&self) -> &'static str {
        "openai"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{ChatMessage, Role};
    use crate::client::LlmConfig;

    fn test_request() -> LlmRequest {
        LlmRequest {
            model: "gpt-4o".into(),
            system_prompt: None,
            prompt: "Why is the sky blue?".into(),
            messages: Vec::new(),
            config: LlmConfig::default(),
            stream: false,
        }
    }

    #[test]
    fn test_openai_backend_chat_payload() {
        let mut request = test_request();
        request.system_prompt = Some("You are a helpful assistant.".into());

        let body = OpenAiBackend::build_body(&request, false);

        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["temperature"], 0.7);
        assert_eq!(body["max_tokens"], 2048);
        assert_eq!(body["stream"], false);

        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are a helpful assistant.");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "Why is the sky blue?");

        // No response_format when json_mode is false
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn test_openai_backend_json_mode() {
        let mut request = test_request();
        request.config.json_mode = true;

        let body = OpenAiBackend::build_body(&request, false);
        let rf = body.get("response_format").expect("response_format");
        assert_eq!(rf["type"], "json_object");
    }

    #[test]
    fn test_openai_backend_no_system() {
        let request = test_request();
        let body = OpenAiBackend::build_body(&request, false);

        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn test_openai_backend_thinking_skipped() {
        let mut request = test_request();
        request.config.thinking = true;

        let body = OpenAiBackend::build_body(&request, false);
        // thinking/extended_thinking should NOT appear in the body
        assert!(body.get("thinking").is_none());
        assert!(body.get("extended_thinking").is_none());
    }

    #[test]
    fn test_openai_backend_custom_options_skipped() {
        let mut request = test_request();
        request.config.options = Some(json!({"top_p": 0.9}));

        let body = OpenAiBackend::build_body(&request, false);
        // Custom Ollama options should not appear
        assert!(body.get("options").is_none());
        assert!(body.get("top_p").is_none());
    }

    #[test]
    fn test_openai_backend_auth_header() {
        let backend = OpenAiBackend::new()
            .with_api_key("sk-test123")
            .with_organization("org-abc");

        let client = Client::new();
        let body = json!({"test": true});
        let req = backend
            .build_http_request(&client, "https://api.openai.com/v1/chat/completions", &body)
            .build()
            .expect("build request");

        let auth = req.headers().get("Authorization").expect("auth header");
        assert_eq!(auth, "Bearer sk-test123");

        let org = req
            .headers()
            .get("OpenAI-Organization")
            .expect("org header");
        assert_eq!(org, "org-abc");
    }

    #[test]
    fn test_openai_backend_no_auth() {
        let backend = OpenAiBackend::new();

        let client = Client::new();
        let body = json!({"test": true});
        let req = backend
            .build_http_request(&client, "https://api.openai.com/v1/chat/completions", &body)
            .build()
            .expect("build request");

        assert!(req.headers().get("Authorization").is_none());
        assert!(req.headers().get("OpenAI-Organization").is_none());
    }

    #[test]
    fn test_openai_backend_streaming_body() {
        let request = test_request();
        let body = OpenAiBackend::build_body(&request, true);
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn test_openai_backend_with_history() {
        let mut request = test_request();
        request.system_prompt = Some("Be helpful.".into());
        request.messages = vec![
            ChatMessage {
                role: Role::User,
                content: "What is 2+2?".into(),
            },
            ChatMessage {
                role: Role::Assistant,
                content: "4".into(),
            },
            ChatMessage {
                role: Role::User,
                content: "And 3+3?".into(),
            },
        ];

        let body = OpenAiBackend::build_body(&request, false);
        let messages = body["messages"].as_array().expect("messages");
        // system + 3 history messages
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["content"], "What is 2+2?");
        assert_eq!(messages[2]["content"], "4");
        assert_eq!(messages[3]["content"], "And 3+3?");
    }

    #[test]
    fn test_debug_redacts_api_key() {
        let backend = OpenAiBackend::new().with_api_key("sk-1234567890abcdef");
        let debug_output = format!("{:?}", backend);
        assert!(
            !debug_output.contains("1234567890abcdef"),
            "API key must not appear in Debug output"
        );
        assert!(
            debug_output.contains("sk-123"),
            "Prefix should be visible for identification"
        );
        assert!(
            debug_output.contains("***"),
            "Redaction marker must be present"
        );
    }

    #[test]
    fn test_debug_no_key() {
        let backend = OpenAiBackend::new();
        let debug_output = format!("{:?}", backend);
        assert!(
            debug_output.contains("None"),
            "No-key case should show None"
        );
    }

    #[test]
    fn test_has_api_key() {
        let without = OpenAiBackend::new();
        assert!(!without.has_api_key());
        let with = OpenAiBackend::new().with_api_key("sk-test");
        assert!(with.has_api_key());
    }

    #[test]
    fn test_has_organization() {
        let without = OpenAiBackend::new();
        assert!(!without.has_organization());
        let with = OpenAiBackend::new().with_organization("org-abc");
        assert!(with.has_organization());
    }
}
