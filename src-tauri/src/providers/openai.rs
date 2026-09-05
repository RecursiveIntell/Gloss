use super::{
    provider_cancelled_error, provider_http_failure, ChatRequest, ChatToken, LlmExecutionContext,
    LlmProvider, ModelInfo, ProviderType,
};
use crate::error::GlossError;
use async_trait::async_trait;
use futures::stream::{self, Stream};
use std::pin::Pin;
use zeroize::Zeroize;

fn parse_stream_event(data: &str) -> Result<Option<ChatToken>, GlossError> {
    parse_compatible_stream_event("openai", data)
}

/// Text completions and explicit refusals are supported; a malformed response
/// or a tool-only answer cannot be represented as a successful empty token.
pub(super) fn parse_compatible_response(
    provider: &str,
    body: &serde_json::Value,
) -> Result<ChatToken, GlossError> {
    let choice = body
        .get("choices")
        .and_then(serde_json::Value::as_array)
        .and_then(|choices| choices.first())
        .filter(|choice| choice.is_object())
        .ok_or_else(|| {
            super::sse::protocol_error(
                provider,
                "Non-stream response requires a nonempty choices array",
            )
        })?;
    if !choice
        .get("finish_reason")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|reason| !reason.is_empty())
    {
        return Err(super::sse::protocol_error(
            provider,
            "Non-stream response is missing a terminal finish_reason",
        ));
    }
    let message = choice
        .get("message")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            super::sse::protocol_error(provider, "Non-stream choice is missing message object")
        })?;
    let content = match message.get("content") {
        Some(serde_json::Value::String(content)) => content,
        Some(serde_json::Value::Null) | None => message.get("refusal")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| super::sse::protocol_error(provider, "Non-stream message has no text or refusal; tool-only responses are unsupported"))?,
        _ => return Err(super::sse::protocol_error(provider, "Non-stream message.content must be a string or null")),
    };
    Ok(ChatToken {
        token: content.to_string(),
        done: true,
    })
}

/// Canonical parser for the OpenAI chat-completion SSE dialect, also used by
/// llama.cpp's compatible endpoint. Framing/lifecycle belongs to `sse`.
pub(super) fn parse_compatible_stream_event(
    provider: &str,
    data: &str,
) -> Result<Option<ChatToken>, GlossError> {
    if data == "[DONE]" {
        return Ok(Some(ChatToken {
            token: String::new(),
            done: true,
        }));
    }
    let value: serde_json::Value = serde_json::from_str(data)
        .map_err(|_| super::sse::protocol_error(provider, "Invalid JSON in SSE event"))?;
    if let Some(error) = value.get("error") {
        let message = error
            .as_str()
            .or_else(|| error.get("message").and_then(|message| message.as_str()))
            .unwrap_or("Provider reported a streaming error");
        return Err(super::sse::protocol_error(
            provider,
            &format!(
                "Provider stream error: {}",
                super::sanitize_provider_error_body(message)
            ),
        ));
    }
    let choices = value
        .get("choices")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            super::sse::protocol_error(provider, "SSE event is missing choices array")
        })?;
    let Some(choice) = choices.first() else {
        // An explicit usage-only chunk contains an empty choices array.
        return if value.get("usage").is_some_and(|usage| usage.is_object()) {
            Ok(None)
        } else {
            Err(super::sse::protocol_error(
                provider,
                "Empty choices without usage metadata",
            ))
        };
    };
    let done = match choice.get("finish_reason") {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::String(reason))
            if matches!(
                reason.as_str(),
                "stop" | "length" | "tool_calls" | "content_filter" | "function_call"
            ) =>
        {
            true
        }
        _ => {
            return Err(super::sse::protocol_error(
                provider,
                "Invalid finish_reason in SSE event",
            ))
        }
    };
    let delta = choice
        .get("delta")
        .and_then(|value| value.as_object())
        .ok_or_else(|| {
            super::sse::protocol_error(provider, "SSE choice is missing delta object")
        })?;
    let content = match delta.get("content") {
        None | Some(serde_json::Value::Null) => "",
        Some(serde_json::Value::String(content)) => content,
        _ => {
            return Err(super::sse::protocol_error(
                provider,
                "Invalid delta.content in SSE event",
            ))
        }
    };
    Ok(Some(ChatToken {
        token: content.to_string(),
        done,
    }))
}

/// OpenAI-compatible LLM provider (also works with OpenAI-compatible APIs).
pub struct OpenAIProvider {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl OpenAIProvider {
    pub fn new(base_url: &str, api_key: &str, client: reqwest::Client) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            client,
        }
    }
}

impl Drop for OpenAIProvider {
    fn drop(&mut self) {
        self.api_key.zeroize();
    }
}

#[async_trait]
impl LlmProvider for OpenAIProvider {
    async fn list_models(&self) -> Result<Vec<ModelInfo>, GlossError> {
        let url = format!("{}/models", self.base_url);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .map_err(|e| GlossError::Provider {
                provider: "openai".into(),
                source: e.into(),
            })?;

        if !resp.status().is_success() {
            return Err(GlossError::Provider {
                provider: "openai".into(),
                source: anyhow::anyhow!("HTTP {}", resp.status()),
            });
        }

        let body: serde_json::Value = resp.json().await.map_err(|e| GlossError::Provider {
            provider: "openai".into(),
            source: e.into(),
        })?;

        let models = body
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let id = m.get("id")?.as_str()?.to_string();
                        Some(ModelInfo {
                            display_name: id.clone(),
                            id,
                            provider: ProviderType::OpenAI,
                            parameter_size: None,
                            context_window: None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(models)
    }

    async fn chat(
        &self,
        request: ChatRequest,
        ctx: LlmExecutionContext,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatToken, GlossError>> + Send>>, GlossError> {
        ctx.check_cancelled("openai", "before_request_build")?;
        let url = format!("{}/chat/completions", self.base_url);

        let mut messages = Vec::new();
        if let Some(ref system) = request.system_prompt {
            messages.push(serde_json::json!({
                "role": "system",
                "content": system,
            }));
        }
        for msg in &request.messages {
            messages.push(serde_json::json!({
                "role": msg.role,
                "content": msg.content,
            }));
        }

        let body = serde_json::json!({
            "model": request.model,
            "messages": messages,
            "stream": request.stream,
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
            "top_p": request.top_p,
        });

        ctx.check_cancelled("openai", "before_http_send")?;
        let send = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send();
        let resp = tokio::select! {
            _ = ctx.cancellation.cancelled() => {
                return Err(provider_cancelled_error("openai", "waiting_for_response_headers", ctx.attempt_id.as_deref()));
            }
            result = send => result.map_err(|e| GlossError::Provider {
                provider: "openai".into(),
                source: e.into(),
            })?
        };

        if !resp.status().is_success() {
            return Err(provider_http_failure("openai", resp, &ctx).await);
        }

        if request.stream {
            Ok(super::sse::response_stream(
                resp,
                ctx,
                "openai",
                parse_stream_event,
            ))
        } else {
            let body = super::bounded_json_response("openai", resp, &ctx).await?;
            let token = parse_compatible_response("openai", &body)?;
            Ok(Box::pin(stream::iter(vec![Ok(token)])))
        }
    }

    async fn health_check(&self) -> Result<bool, GlossError> {
        let url = format!("{}/models", self.base_url);
        match self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
        {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    fn provider_type(&self) -> ProviderType {
        ProviderType::OpenAI
    }
}

#[cfg(test)]
mod tests {
    use super::OpenAIProvider;
    use crate::providers::build_shared_client;

    #[test]
    fn shared_client_pool_reuses_connections() {
        let client = build_shared_client().expect("provider HTTP client initializes");
        let p1 = OpenAIProvider::new("http://x", "k", client.clone());
        let p2 = OpenAIProvider::new("http://x", "k", client.clone());
        let _ = (p1, p2);
    }

    #[test]
    fn compatible_stream_requires_typed_content_and_known_terminal_reason() {
        for wire in [
            r#"{"choices":[{"delta":{"content":7},"finish_reason":null}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":true}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"unknown"}]}"#,
            r#"{"choices":[{"finish_reason":"stop"}]}"#,
            r#"{"choices":[]}"#,
        ] {
            assert!(super::parse_compatible_stream_event("openai", wire).is_err());
        }
        assert!(super::parse_compatible_stream_event(
            "openai",
            r#"{"choices":[],"usage":{"total_tokens":4}}"#
        )
        .unwrap()
        .is_none());
        assert!(!super::parse_compatible_stream_event("openai", r#"{"choices":[{"delta":{"role":"assistant","content":null},"finish_reason":null}]}"#).unwrap().unwrap().done);
    }
}
