use super::{ChatRequest, ChatToken, LlmProvider, ModelInfo, ProviderType};
use crate::error::GlossError;
use async_trait::async_trait;
use futures::stream::{self, Stream};
use futures::StreamExt;
use std::pin::Pin;

/// llama.cpp server provider (OpenAI-compatible API).
pub struct LlamaCppProvider {
    base_url: String,
    client: reqwest::Client,
}

impl LlamaCppProvider {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }
}

#[async_trait]
impl LlmProvider for LlamaCppProvider {
    async fn list_models(&self) -> Result<Vec<ModelInfo>, GlossError> {
        let url = format!("{}/models", self.base_url);
        let resp = self.client.get(&url).send().await;

        match resp {
            Ok(r) if r.status().is_success() => {
                let body: serde_json::Value = r.json().await.map_err(|e| GlossError::Provider {
                    provider: "llamacpp".into(),
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
                                    provider: ProviderType::LlamaCpp,
                                    parameter_size: None,
                                    context_window: None,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                Ok(models)
            }
            _ => {
                // Older llama.cpp versions may not support /models — return placeholder
                Ok(vec![ModelInfo {
                    id: "llama.cpp-loaded-model".into(),
                    display_name: "llama.cpp (loaded model)".into(),
                    provider: ProviderType::LlamaCpp,
                    parameter_size: None,
                    context_window: None,
                }])
            }
        }
    }

    async fn chat(
        &self,
        request: ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatToken, GlossError>> + Send>>, GlossError> {
        // llama.cpp uses OpenAI-compatible /chat/completions endpoint
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
        });

        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| GlossError::Provider {
                provider: "llamacpp".into(),
                source: e.into(),
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(GlossError::Provider {
                provider: "llamacpp".into(),
                source: anyhow::anyhow!("HTTP {}: {}", status, text),
            });
        }

        if request.stream {
            // SSE parsing identical to OpenAI
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
                                        let data = data.trim();
                                        if data == "[DONE]" {
                                            tokens.push(Ok(ChatToken {
                                                token: String::new(),
                                                done: true,
                                            }));
                                            break;
                                        }
                                        if let Ok(val) =
                                            serde_json::from_str::<serde_json::Value>(data)
                                        {
                                            let content = val
                                                .get("choices")
                                                .and_then(|c| c.get(0))
                                                .and_then(|c| c.get("delta"))
                                                .and_then(|d| d.get("content"))
                                                .and_then(|c| c.as_str())
                                                .unwrap_or("")
                                                .to_string();
                                            let finish = val
                                                .get("choices")
                                                .and_then(|c| c.get(0))
                                                .and_then(|c| c.get("finish_reason"))
                                                .and_then(|f| f.as_str())
                                                .is_some();
                                            tokens.push(Ok(ChatToken {
                                                token: content,
                                                done: finish,
                                            }));
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
                                        provider: "llamacpp".into(),
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
                provider: "llamacpp".into(),
                source: e.into(),
            })?;

            let content = body
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("message"))
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();

            Ok(Box::pin(stream::iter(vec![Ok(ChatToken {
                token: content,
                done: true,
            })])))
        }
    }

    async fn health_check(&self) -> Result<bool, GlossError> {
        // Try /health first (llama.cpp), then /models (OpenAI compat)
        let health_url = format!("{}/health", self.base_url.trim_end_matches("/v1"));
        match self.client.get(&health_url).send().await {
            Ok(resp) if resp.status().is_success() => return Ok(true),
            _ => {}
        }
        let models_url = format!("{}/models", self.base_url);
        match self.client.get(&models_url).send().await {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    fn provider_type(&self) -> ProviderType {
        ProviderType::LlamaCpp
    }
}
