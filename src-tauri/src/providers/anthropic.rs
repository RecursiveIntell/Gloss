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
    let value: serde_json::Value = serde_json::from_str(data)
        .map_err(|_| super::sse::protocol_error("anthropic", "Invalid JSON in SSE event"))?;
    if let Some(error) = value.get("error") {
        let message = error
            .as_str()
            .or_else(|| error.get("message").and_then(|message| message.as_str()))
            .unwrap_or("Provider reported a streaming error");
        return Err(super::sse::protocol_error(
            "anthropic",
            &format!(
                "Provider stream error: {}",
                super::sanitize_provider_error_body(message)
            ),
        ));
    }
    let kind = value
        .get("type")
        .and_then(|value| value.as_str())
        .ok_or_else(|| super::sse::protocol_error("anthropic", "SSE event is missing type"))?;
    match kind {
        "message_stop" => Ok(Some(ChatToken {
            token: String::new(),
            done: true,
        })),
        "content_block_delta" => {
            let delta = value
                .get("delta")
                .and_then(|value| value.as_object())
                .ok_or_else(|| {
                    super::sse::protocol_error("anthropic", "Content delta is missing delta object")
                })?;
            let kind = delta
                .get("type")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    super::sse::protocol_error("anthropic", "Content delta is missing delta type")
                })?;
            if kind != "text_delta" {
                return Ok(None); // thinking, signatures and tool JSON are not answer text
            }
            let text = delta
                .get("text")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    super::sse::protocol_error("anthropic", "Text delta is missing text string")
                })?;
            Ok(Some(ChatToken {
                token: text.to_string(),
                done: false,
            }))
        }
        "error" => Err(super::sse::protocol_error(
            "anthropic",
            "Provider sent malformed error event",
        )),
        // The API explicitly permits added event types. Metadata, usage and ping
        // events do not constitute completion; only message_stop does.
        _ => Ok(None),
    }
}

fn parse_response(body: &serde_json::Value) -> Result<ChatToken, GlossError> {
    let error = |message| super::sse::protocol_error("anthropic", message);
    if body.get("type").and_then(serde_json::Value::as_str) != Some("message") {
        return Err(error("Non-stream response is missing message type"));
    }
    if !body
        .get("stop_reason")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|reason| !reason.is_empty())
    {
        return Err(error(
            "Non-stream response is missing a terminal stop_reason",
        ));
    }
    let blocks = body
        .get("content")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| error("Non-stream response is missing content array"))?;
    let mut text = String::new();
    let mut saw_text = false;
    for block in blocks {
        let kind = block
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| error("Non-stream content block is missing type"))?;
        if kind == "text" {
            text.push_str(
                block
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| error("Non-stream text block is missing text string"))?,
            );
            saw_text = true;
        }
        // Thinking, signatures and tool blocks are not answer text. Other
        // typed blocks can coexist with text without discarding the answer.
    }
    if !saw_text {
        return Err(error("Non-stream response has no text content block"));
    }
    Ok(ChatToken {
        token: text,
        done: true,
    })
}

/// Anthropic LLM provider.
pub struct AnthropicProvider {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(base_url: &str, api_key: &str, client: reqwest::Client) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            client,
        }
    }
}

impl Drop for AnthropicProvider {
    fn drop(&mut self) {
        self.api_key.zeroize();
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn list_models(&self) -> Result<Vec<ModelInfo>, GlossError> {
        let url = format!("{}/models", self.base_url);
        let resp = self
            .client
            .get(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
            .map_err(|e| GlossError::Provider {
                provider: "anthropic".into(),
                source: e.into(),
            })?;

        if !resp.status().is_success() {
            // If model listing is unavailable, return well-known models
            return Ok(vec![
                ModelInfo {
                    id: "claude-sonnet-4-5-20250929".into(),
                    display_name: "Claude Sonnet 4.5".into(),
                    provider: ProviderType::Anthropic,
                    parameter_size: None,
                    context_window: Some(200000),
                },
                ModelInfo {
                    id: "claude-haiku-4-5-20251001".into(),
                    display_name: "Claude Haiku 4.5".into(),
                    provider: ProviderType::Anthropic,
                    parameter_size: None,
                    context_window: Some(200000),
                },
            ]);
        }

        let body: serde_json::Value = resp.json().await.map_err(|e| GlossError::Provider {
            provider: "anthropic".into(),
            source: e.into(),
        })?;

        let models = body
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let id = m.get("id")?.as_str()?.to_string();
                        let display = m
                            .get("display_name")
                            .and_then(|n| n.as_str())
                            .unwrap_or(&id)
                            .to_string();
                        Some(ModelInfo {
                            display_name: display,
                            id,
                            provider: ProviderType::Anthropic,
                            parameter_size: None,
                            context_window: Some(200000),
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
        ctx.check_cancelled("anthropic", "before_request_build")?;
        let url = format!("{}/messages", self.base_url);

        // Anthropic: system goes as top-level field, NOT a message role
        let messages: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(|msg| {
                serde_json::json!({
                    "role": msg.role,
                    "content": msg.content,
                })
            })
            .collect();

        let mut body = serde_json::json!({
            "model": request.model,
            "messages": messages,
            "max_tokens": request.max_tokens,
            "stream": request.stream,
        });

        if let Some(ref system) = request.system_prompt {
            body["system"] = serde_json::json!(system);
        }

        ctx.check_cancelled("anthropic", "before_http_send")?;
        let send = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send();
        let resp = tokio::select! {
            _ = ctx.cancellation.cancelled() => {
                return Err(provider_cancelled_error("anthropic", "waiting_for_response_headers", ctx.attempt_id.as_deref()));
            }
            result = send => result.map_err(|e| GlossError::Provider {
                provider: "anthropic".into(),
                source: e.into(),
            })?
        };

        if !resp.status().is_success() {
            return Err(provider_http_failure("anthropic", resp, &ctx).await);
        }

        if request.stream {
            Ok(super::sse::response_stream(
                resp,
                ctx,
                "anthropic",
                parse_stream_event,
            ))
        } else {
            let body = super::bounded_json_response("anthropic", resp, &ctx).await?;
            let token = parse_response(&body)?;
            Ok(Box::pin(stream::iter(vec![Ok(token)])))
        }
    }

    async fn health_check(&self) -> Result<bool, GlossError> {
        let url = format!("{}/models", self.base_url);
        match self
            .client
            .get(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
        {
            // 401 means the key is wrong or missing — not healthy
            Ok(resp) => Ok(resp.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    fn provider_type(&self) -> ProviderType {
        ProviderType::Anthropic
    }
}
