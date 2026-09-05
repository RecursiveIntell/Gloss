use super::{
    provider_cancelled_error, provider_http_failure, ChatRequest, ChatToken, LlmExecutionContext,
    LlmProvider, ModelInfo, ProviderType,
};
use crate::error::GlossError;
use async_trait::async_trait;
use futures::stream::{self, Stream};
use std::pin::Pin;

fn parse_stream_event(data: &str) -> Result<Option<ChatToken>, GlossError> {
    super::openai::parse_compatible_stream_event("llamacpp", data)
}

/// llama.cpp server provider (OpenAI-compatible API).
pub struct LlamaCppProvider {
    base_url: String,
    client: reqwest::Client,
}

impl LlamaCppProvider {
    pub fn new(base_url: &str, client: reqwest::Client) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
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
        ctx: LlmExecutionContext,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatToken, GlossError>> + Send>>, GlossError> {
        ctx.check_cancelled("llamacpp", "before_request_build")?;
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
            "top_p": request.top_p,
        });

        ctx.check_cancelled("llamacpp", "before_http_send")?;
        let send = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send();
        let resp = tokio::select! {
            _ = ctx.cancellation.cancelled() => {
                return Err(provider_cancelled_error("llamacpp", "waiting_for_response_headers", ctx.attempt_id.as_deref()));
            }
            result = send => result.map_err(|e| GlossError::Provider {
                provider: "llamacpp".into(),
                source: e.into(),
            })?
        };

        if !resp.status().is_success() {
            return Err(provider_http_failure("llamacpp", resp, &ctx).await);
        }

        if request.stream {
            Ok(super::sse::response_stream(
                resp,
                ctx,
                "llamacpp",
                parse_stream_event,
            ))
        } else {
            let body = super::bounded_json_response("llamacpp", resp, &ctx).await?;
            let token = super::openai::parse_compatible_response("llamacpp", &body)?;
            Ok(Box::pin(stream::iter(vec![Ok(token)])))
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
