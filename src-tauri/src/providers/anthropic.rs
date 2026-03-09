use super::{ChatRequest, ChatToken, LlmProvider, ModelInfo, ProviderType};
use crate::error::GlossError;
use async_trait::async_trait;
use futures::stream::{self, Stream};
use futures::StreamExt;
use std::pin::Pin;

/// Anthropic LLM provider.
pub struct AnthropicProvider {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
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
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatToken, GlossError>> + Send>>, GlossError> {
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

        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| GlossError::Provider {
                provider: "anthropic".into(),
                source: e.into(),
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(GlossError::Provider {
                provider: "anthropic".into(),
                source: anyhow::anyhow!("HTTP {}: {}", status, text),
            });
        }

        if request.stream {
            let byte_stream = resp.bytes_stream();

            let stream = stream::unfold(
                (byte_stream, String::new()),
                |(mut byte_stream, mut buffer)| async move {
                    loop {
                        match byte_stream.next().await {
                            Some(Ok(bytes)) => {
                                buffer.push_str(&String::from_utf8_lossy(&bytes));
                                let mut tokens: Vec<Result<ChatToken, GlossError>> = Vec::new();

                                while let Some(newline_pos) = buffer.find('\n') {
                                    let line = buffer[..newline_pos].trim().to_string();
                                    buffer = buffer[newline_pos + 1..].to_string();

                                    if line.is_empty() || line.starts_with(':') {
                                        continue;
                                    }
                                    if let Some(data) = line.strip_prefix("data: ") {
                                        if let Ok(val) =
                                            serde_json::from_str::<serde_json::Value>(data)
                                        {
                                            let event_type = val
                                                .get("type")
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("");

                                            match event_type {
                                                "content_block_delta" => {
                                                    let text = val
                                                        .get("delta")
                                                        .and_then(|d| d.get("text"))
                                                        .and_then(|t| t.as_str())
                                                        .unwrap_or("")
                                                        .to_string();
                                                    tokens.push(Ok(ChatToken {
                                                        token: text,
                                                        done: false,
                                                    }));
                                                }
                                                "message_stop" => {
                                                    tokens.push(Ok(ChatToken {
                                                        token: String::new(),
                                                        done: true,
                                                    }));
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                }

                                if !tokens.is_empty() {
                                    return Some((stream::iter(tokens), (byte_stream, buffer)));
                                }
                            }
                            Some(Err(e)) => {
                                return Some((
                                    stream::iter(vec![Err(GlossError::Provider {
                                        provider: "anthropic".into(),
                                        source: e.into(),
                                    })]),
                                    (byte_stream, buffer),
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
            let body: serde_json::Value = resp.json().await.map_err(|e| GlossError::Provider {
                provider: "anthropic".into(),
                source: e.into(),
            })?;

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
            Ok(resp) => Ok(resp.status().is_success() || resp.status().as_u16() == 401),
            Err(_) => Ok(false),
        }
    }

    fn provider_type(&self) -> ProviderType {
        ProviderType::Anthropic
    }
}
