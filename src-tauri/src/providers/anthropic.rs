use super::{
    provider_cancelled_error, provider_http_error, ChatRequest, ChatToken, LlmExecutionContext,
    LlmProvider, ModelInfo, ProviderType,
};
use crate::error::GlossError;
use async_trait::async_trait;
use futures::stream::{self, Stream};
use futures::StreamExt;
use std::pin::Pin;
use zeroize::Zeroize;

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
            let status = resp.status();
            // F3: bound the error body to ~1KB so a hostile / misconfigured
            // server can't fill the logs with megabytes of HTML or stack
            // traces. Use bytes (cheap) instead of text (allocates).
            let text = {
                match resp.bytes().await {
                    Ok(b) => String::from_utf8_lossy(&b[..b.len().min(1024)]).to_string(),
                    Err(_) => String::new(),
                }
            };
            return Err(provider_http_error("anthropic", status, &text));
        }

        if request.stream {
            let byte_stream = resp.bytes_stream();

            let stream = stream::unfold(
                (byte_stream, super::sse::SseDecoder::new(), ctx.clone()),
                |(mut byte_stream, mut buffer, ctx)| async move {
                    loop {
                        let next = tokio::select! {
                            _ = ctx.cancellation.cancelled() => {
                                return Some((
                                    stream::iter(vec![Err(provider_cancelled_error("anthropic", "reading_stream_chunk", ctx.attempt_id.as_deref()))]),
                                    (byte_stream, buffer, ctx),
                                ));
                            }
                            next = byte_stream.next() => next,
                        };
                        match next {
                            Some(Ok(bytes)) => {
                                if ctx.is_cancelled() {
                                    return Some((
                                        stream::iter(vec![Err(provider_cancelled_error("anthropic", "before_yield_token", ctx.attempt_id.as_deref()))]),
                                        (byte_stream, buffer, ctx),
                                    ));
                                }
                                let mut tokens: Vec<Result<ChatToken, GlossError>> = Vec::new();
                                let events = match buffer.push(&bytes) {
                                    Ok(events) => events,
                                    Err(error) => return Some((
                                        stream::iter(vec![Err(GlossError::Provider { provider: "anthropic".into(), source: anyhow::anyhow!(error) })]),
                                        (byte_stream, buffer, ctx),
                                    )),
                                };
                                for data in events {
                                    let val: serde_json::Value = match serde_json::from_str(&data) {
                                        Ok(val) => val,
                                        Err(error) => {
                                            tokens.push(Err(GlossError::Provider { provider: "anthropic".into(), source: anyhow::anyhow!("Invalid JSON in SSE event: {}", error) })); break;
                                        }
                                    };
                                    if let Some(error) = val.get("error") {
                                        let message = error.as_str().or_else(|| error.get("message").and_then(|v| v.as_str())).unwrap_or("Provider reported a streaming error");
                                        let bounded: String = message.chars().take(512).collect();
                                        tokens.push(Err(GlossError::Provider { provider: "anthropic".into(), source: anyhow::anyhow!("Provider stream error: {}", bounded) })); break;
                                    }
                                    match val.get("type").and_then(|v| v.as_str()).unwrap_or("") {
                                        "content_block_delta" => tokens.push(Ok(ChatToken {
                                            token: val.get("delta").and_then(|v| v.get("text")).and_then(|v| v.as_str()).unwrap_or("").to_string(), done: false,
                                        })),
                                        "message_stop" => {
                                            tokens.push(Ok(ChatToken { token: String::new(), done: true })); break;
                                        }
                                        _ => {} // metadata, usage, and ping events carry no text
                                    }
                                }

                                if !tokens.is_empty() {
                                    if ctx.is_cancelled() {
                                        return Some((
                                            stream::iter(vec![Err(provider_cancelled_error("anthropic", "before_yield_token", ctx.attempt_id.as_deref()))]),
                                            (byte_stream, buffer, ctx),
                                        ));
                                    }
                                    return Some((stream::iter(tokens), (byte_stream, buffer, ctx)));
                                }
                            }
                            Some(Err(e)) => {
                                return Some((
                                    stream::iter(vec![Err(GlossError::Provider {
                                        provider: "anthropic".into(),
                                        source: e.into(),
                                    })]),
                                    (byte_stream, buffer, ctx),
                                ));
                            }
                            None => {
                                return None;
                            }
                        }
                    }
                },
            )
            .flatten();

            Ok(Box::pin(stream))
        } else {
            let body: serde_json::Value = tokio::select! {
                _ = ctx.cancellation.cancelled() => {
                    return Err(provider_cancelled_error("anthropic", "reading_non_stream_response", ctx.attempt_id.as_deref()));
                }
                result = resp.json() => result.map_err(|e| GlossError::Provider {
                provider: "anthropic".into(),
                source: e.into(),
                })?
            };
            ctx.check_cancelled("anthropic", "before_terminal_frame")?;

            let content = body
                .get("content")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();

            Ok(Box::pin(stream::iter(vec![Ok(ChatToken {
                token: content,
                done: true,
            })])))
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
