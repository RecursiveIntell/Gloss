use super::{provider_http_error, ChatRequest, ChatToken, LlmProvider, ModelInfo, ProviderType};
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
    pub fn new(base_url: &str, client: reqwest::Client) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
        }
    }
}

fn ollama_chat_token_from_value(val: &serde_json::Value) -> Result<ChatToken, GlossError> {
    if let Some(error) = val.get("error").and_then(|error| error.as_str()) {
        return Err(GlossError::Provider {
            provider: "ollama".into(),
            source: anyhow::anyhow!("Ollama stream error: {error}"),
        });
    }

    let done = val.get("done").and_then(|d| d.as_bool()).unwrap_or(false);
    let token = val
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    Ok(ChatToken { token, done })
}

fn build_ollama_chat_body(request: &ChatRequest) -> serde_json::Value {
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
    if let Some(top_p) = request.top_p {
        options["top_p"] = serde_json::json!(top_p);
    }
    if let Some(top_k) = request.top_k {
        options["top_k"] = serde_json::json!(top_k);
    }
    if let Some(min_p) = request.min_p {
        options["min_p"] = serde_json::json!(min_p);
    }
    if let Some(repeat_penalty) = request.repeat_penalty {
        options["repeat_penalty"] = serde_json::json!(repeat_penalty);
    }
    if let Some(num_ctx) = request.num_ctx {
        options["num_ctx"] = serde_json::json!(num_ctx);
    }

    serde_json::json!({
        "model": request.model,
        "messages": messages,
        "stream": request.stream,
        "think": false,
        "options": options,
    })
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
        let body = build_ollama_chat_body(&request);

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
            // F3: bound the error body to ~1KB so a hostile / misconfigured
            // server can't fill the logs with megabytes of HTML or stack
            // traces. Use bytes (cheap) instead of text (allocates).
            let text = {
                match resp.bytes().await {
                    Ok(b) => String::from_utf8_lossy(&b[..b.len().min(1024)]).to_string(),
                    Err(_) => String::new(),
                }
            };
            return Err(provider_http_error("ollama", status, &text));
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
                                    tokens.push(ollama_chat_token_from_value(&val));
                                }
                                if !tokens.is_empty() {
                                    return Some((stream::iter(tokens), (byte_stream, decoder)));
                                }
                            }
                            Ok(None) => {
                                // Stream ended — flush decoder
                                if let Some(val) = decoder.flush() {
                                    return Some((
                                        stream::iter(vec![ollama_chat_token_from_value(&val)]),
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

            if let Some(error) = body.get("error").and_then(|error| error.as_str()) {
                return Err(GlossError::Provider {
                    provider: "ollama".into(),
                    source: anyhow::anyhow!("Ollama response error: {error}"),
                });
            }

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
        // Hit /api/tags instead of root — verifies Ollama can list models,
        // which is the meaningful availability check for chat.
        let url = format!("{}/api/tags", self.base_url);
        match self.client.get(&url).send().await {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    fn provider_type(&self) -> ProviderType {
        ProviderType::Ollama
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ChatMessage;

    fn smoke_request() -> ChatRequest {
        ChatRequest {
            model: "qwen3.5:4b".to_string(),
            system_prompt: Some("system".to_string()),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "Reply exactly: gloss smoke ok".to_string(),
                images: None,
            }],
            max_tokens: 64,
            temperature: 0.0,
            top_p: None,
            top_k: None,
            min_p: None,
            repeat_penalty: None,
            stream: true,
            num_ctx: Some(8192),
        }
    }

    #[test]
    fn ollama_chat_body_disables_thinking_for_visible_content() {
        let body = build_ollama_chat_body(&smoke_request());
        assert_eq!(
            body.get("think").and_then(|value| value.as_bool()),
            Some(false)
        );
        assert_eq!(
            body.get("options")
                .and_then(|options| options.get("num_ctx"))
                .and_then(|value| value.as_u64()),
            Some(8192)
        );
        assert_eq!(
            body.get("messages")
                .and_then(|messages| messages.as_array())
                .map(Vec::len),
            Some(2)
        );
    }

    #[test]
    fn ollama_stream_error_frame_becomes_provider_error() {
        let value = serde_json::json!({
            "error": "model not found"
        });
        let err = ollama_chat_token_from_value(&value).expect_err("error frame must fail");
        assert!(err
            .to_string()
            .contains("Ollama stream error: model not found"));
    }

    #[test]
    fn ollama_normal_frame_extracts_content_and_done() {
        let value = serde_json::json!({
            "message": {"content": "gloss smoke ok"},
            "done": true
        });
        let token = ollama_chat_token_from_value(&value).expect("normal frame should parse");
        assert_eq!(token.token, "gloss smoke ok");
        assert!(token.done);
    }
}
