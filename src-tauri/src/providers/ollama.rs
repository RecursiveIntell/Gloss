use super::{ChatRequest, ChatToken, LlmProvider, ModelInfo, ProviderType};
use crate::error::GlossError;
use async_trait::async_trait;
use futures::stream::{self, Stream};
use std::pin::Pin;

/// Ollama LLM provider implementation.
pub struct OllamaProvider {
    base_url: String,
    client: reqwest::Client,
}

impl OllamaProvider {
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
impl LlmProvider for OllamaProvider {
    async fn list_models(&self) -> Result<Vec<ModelInfo>, GlossError> {
        let url = format!("{}/api/tags", self.base_url);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| GlossError::Provider {
                provider: "ollama".into(),
                source: e.into(),
            })?;

        if !resp.status().is_success() {
            return Err(GlossError::Provider {
                provider: "ollama".into(),
                source: anyhow::anyhow!("HTTP {}", resp.status()),
            });
        }

        let body: serde_json::Value = resp.json().await.map_err(|e| GlossError::Provider {
            provider: "ollama".into(),
            source: e.into(),
        })?;

        let models = body
            .get("models")
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let name = m.get("name")?.as_str()?.to_string();
                        let display = m
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or(&name)
                            .to_string();
                        let param_size = m
                            .get("details")
                            .and_then(|d| d.get("parameter_size"))
                            .and_then(|p| p.as_str())
                            .map(|s| s.to_string());
                        Some(ModelInfo {
                            id: name,
                            display_name: display,
                            provider: ProviderType::Ollama,
                            parameter_size: param_size,
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
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatToken, GlossError>> + Send>>, GlossError> {
        let url = format!("{}/api/chat", self.base_url);

        // Build messages array
        let mut messages = Vec::new();
        if let Some(ref system) = request.system_prompt {
            messages.push(serde_json::json!({
                "role": "system",
                "content": system,
            }));
        }
        for msg in &request.messages {
            let mut msg_json = serde_json::json!({
                "role": msg.role,
                "content": msg.content,
            });
            if let Some(ref images) = msg.images {
                msg_json["images"] = serde_json::json!(images);
            }
            messages.push(msg_json);
        }

        let mut options = serde_json::json!({
            "temperature": request.temperature,
            "num_predict": request.max_tokens,
        });
        if let Some(num_ctx) = request.num_ctx {
            options["num_ctx"] = serde_json::json!(num_ctx);
        }

        let body = serde_json::json!({
            "model": request.model,
            "messages": messages,
            "stream": request.stream,
            "options": options,
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| GlossError::Provider {
                provider: "ollama".into(),
                source: e.into(),
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(GlossError::Provider {
                provider: "ollama".into(),
                source: anyhow::anyhow!("HTTP {}: {}", status, text),
            });
        }

        if request.stream {
            // Streaming: parse NDJSON stream
            let byte_stream = resp.bytes_stream();
            use futures::StreamExt;
            use llm_pipeline::StreamingDecoder;

            let stream = stream::unfold(
                (byte_stream, StreamingDecoder::new()),
                |(mut byte_stream, mut decoder)| async move {
                    use futures::TryStreamExt;
                    loop {
                        match byte_stream.try_next().await {
                            Ok(Some(bytes)) => {
                                let values = decoder.decode(&bytes);
                                let mut tokens: Vec<Result<ChatToken, GlossError>> = Vec::new();
                                for val in values {
                                    let done =
                                        val.get("done").and_then(|d| d.as_bool()).unwrap_or(false);
                                    let token = val
                                        .get("message")
                                        .and_then(|m| m.get("content"))
                                        .and_then(|c| c.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    tokens.push(Ok(ChatToken { token, done }));
                                }
                                if !tokens.is_empty() {
                                    return Some((stream::iter(tokens), (byte_stream, decoder)));
                                }
                            }
                            Ok(None) => {
                                // Stream ended — flush decoder
                                if let Some(val) = decoder.flush() {
                                    let done =
                                        val.get("done").and_then(|d| d.as_bool()).unwrap_or(true);
                                    let token = val
                                        .get("message")
                                        .and_then(|m| m.get("content"))
                                        .and_then(|c| c.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    return Some((
                                        stream::iter(vec![Ok(ChatToken { token, done })]),
                                        (byte_stream, decoder),
                                    ));
                                }
                                return None;
                            }
                            Err(e) => {
                                return Some((
                                    stream::iter(vec![Err(GlossError::Provider {
                                        provider: "ollama".into(),
                                        source: e.into(),
                                    })]),
                                    (byte_stream, decoder),
                                ));
                            }
                        }
                    }
                },
            )
            .flatten();

            Ok(Box::pin(stream))
        } else {
            // Non-streaming: parse single response
            let body: serde_json::Value = resp.json().await.map_err(|e| GlossError::Provider {
                provider: "ollama".into(),
                source: e.into(),
            })?;

            let content = body
                .get("message")
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
        let url = format!("{}/", self.base_url);
        match self.client.get(&url).send().await {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    fn provider_type(&self) -> ProviderType {
        ProviderType::Ollama
    }
}
