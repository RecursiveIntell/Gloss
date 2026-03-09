use crate::{
    error::Result, parsing, streaming::StreamingDecoder, types::StageOutput, PipelineError,
};
use futures::StreamExt;
use reqwest::Client;
use serde_json::{json, Value};

/// Configuration for LLM requests.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// Temperature (0.0 = deterministic, 1.0 = creative).
    pub temperature: f64,

    /// Maximum tokens to generate.
    pub max_tokens: u32,

    /// Enable extended thinking mode (DeepSeek R1 style `<think>` tags).
    pub thinking: bool,

    /// Request JSON format output from the model.
    pub json_mode: bool,

    /// Custom options merged into the Ollama options object.
    pub options: Option<Value>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            temperature: 0.7,
            max_tokens: 2048,
            thinking: false,
            json_mode: false,
            options: None,
        }
    }
}

impl LlmConfig {
    pub fn with_temperature(mut self, temp: f64) -> Self {
        self.temperature = temp;
        self
    }

    pub fn with_max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = tokens;
        self
    }

    pub fn with_thinking(mut self, enabled: bool) -> Self {
        self.thinking = enabled;
        self
    }

    pub fn with_json_mode(mut self, enabled: bool) -> Self {
        self.json_mode = enabled;
        self
    }
}

/// Call LLM with `/api/generate` and parse the response into `T`.
///
/// # Deprecated
/// Use [`LlmCall`](crate::LlmCall) with an [`ExecCtx`](crate::ExecCtx) instead,
/// which supports multiple backends, output strategies, and retry.
#[deprecated(
    since = "0.1.0",
    note = "Use LlmCall with ExecCtx instead. See LlmCall docs for migration."
)]
pub async fn call_llm<T>(
    client: &Client,
    endpoint: &str,
    model: &str,
    prompt: &str,
    config: &LlmConfig,
) -> Result<StageOutput<T>>
where
    T: serde::de::DeserializeOwned,
{
    let mut body = json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        "options": {
            "temperature": config.temperature,
            "num_predict": config.max_tokens,
        },
    });

    if config.thinking {
        body["options"]["extended_thinking"] = json!(true);
    }

    if config.json_mode {
        body["format"] = json!("json");
    }

    merge_custom_options(&mut body, config);

    let url = format!("{}/api/generate", endpoint.trim_end_matches('/'));
    let resp =
        client.post(&url).json(&body).send().await.map_err(|e| {
            PipelineError::Other(format!("Failed to connect to LLM at {}: {}", url, e))
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(PipelineError::Other(format!(
            "LLM returned error {}: {}",
            status, text
        )));
    }

    let json_response: Value = resp.json().await?;
    let raw_response = json_response
        .get("response")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let (thinking, cleaned_response) = parsing::extract_thinking(&raw_response);
    let output: T = parsing::parse_as(&cleaned_response)?;

    Ok(StageOutput {
        output,
        thinking,
        raw_response,
    })
}

/// Call LLM with `/api/chat` (supports system messages) and parse the response.
///
/// # Deprecated
/// Use [`LlmCall`](crate::LlmCall) with an [`ExecCtx`](crate::ExecCtx) instead,
/// which supports system prompts via the stage's `system_prompt` field.
#[deprecated(
    since = "0.1.0",
    note = "Use LlmCall with ExecCtx instead. See LlmCall docs for migration."
)]
pub async fn call_llm_chat<T>(
    client: &Client,
    endpoint: &str,
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
    config: &LlmConfig,
) -> Result<StageOutput<T>>
where
    T: serde::de::DeserializeOwned,
{
    let mut messages = vec![];
    if !system_prompt.is_empty() {
        messages.push(json!({"role": "system", "content": system_prompt}));
    }
    messages.push(json!({"role": "user", "content": user_prompt}));

    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "options": {
            "temperature": config.temperature,
            "num_predict": config.max_tokens,
        },
    });

    if config.thinking {
        body["options"]["extended_thinking"] = json!(true);
    }

    if config.json_mode {
        body["format"] = json!("json");
    }

    merge_custom_options(&mut body, config);

    let url = format!("{}/api/chat", endpoint.trim_end_matches('/'));
    let resp =
        client.post(&url).json(&body).send().await.map_err(|e| {
            PipelineError::Other(format!("Failed to connect to LLM at {}: {}", url, e))
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(PipelineError::Other(format!(
            "LLM returned error {}: {}",
            status, text
        )));
    }

    let json_response: Value = resp.json().await?;
    let raw_response = json_response
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let (thinking, cleaned_response) = parsing::extract_thinking(&raw_response);
    let output: T = parsing::parse_as(&cleaned_response)?;

    Ok(StageOutput {
        output,
        thinking,
        raw_response,
    })
}

/// Call LLM with `/api/generate` in streaming mode, invoking `on_chunk` for each token.
///
/// Uses buffered line-framing to correctly handle JSON lines split across
/// chunk boundaries.
///
/// # Deprecated
/// Use [`LlmCall`](crate::LlmCall) with streaming enabled instead, which
/// supports multiple backends and streaming output strategies.
#[deprecated(
    since = "0.1.0",
    note = "Use LlmCall with ExecCtx and streaming instead. See LlmCall docs for migration."
)]
pub async fn call_llm_streaming<T, F>(
    client: &Client,
    endpoint: &str,
    model: &str,
    prompt: &str,
    config: &LlmConfig,
    mut on_chunk: F,
) -> Result<StageOutput<T>>
where
    T: serde::de::DeserializeOwned,
    F: FnMut(&str),
{
    let mut body = json!({
        "model": model,
        "prompt": prompt,
        "stream": true,
        "options": {
            "temperature": config.temperature,
            "num_predict": config.max_tokens,
        },
    });

    if config.thinking {
        body["options"]["extended_thinking"] = json!(true);
    }

    if config.json_mode {
        body["format"] = json!("json");
    }

    merge_custom_options(&mut body, config);

    let url = format!("{}/api/generate", endpoint.trim_end_matches('/'));
    let resp =
        client.post(&url).json(&body).send().await.map_err(|e| {
            PipelineError::Other(format!("Failed to connect to LLM at {}: {}", url, e))
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(PipelineError::Other(format!(
            "LLM returned error {}: {}",
            status, text
        )));
    }

    let mut stream = resp.bytes_stream();
    let mut decoder = StreamingDecoder::new();
    let mut accumulated = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(PipelineError::Request)?;
        for json_val in decoder.decode(&chunk) {
            if let Some(response) = json_val.get("response").and_then(|v| v.as_str()) {
                accumulated.push_str(response);
                on_chunk(response);
            }
        }
    }

    // Flush remaining buffer
    if let Some(json_val) = decoder.flush() {
        if let Some(response) = json_val.get("response").and_then(|v| v.as_str()) {
            accumulated.push_str(response);
            on_chunk(response);
        }
    }

    let (thinking, cleaned) = parsing::extract_thinking(&accumulated);
    let output: T = parsing::parse_as(&cleaned)?;

    Ok(StageOutput {
        output,
        thinking,
        raw_response: accumulated,
    })
}

/// Merge custom options into the body's options object.
fn merge_custom_options(body: &mut Value, config: &LlmConfig) {
    if let Some(ref opts) = config.options {
        if let Some(options) = body["options"].as_object_mut() {
            if let Some(custom) = opts.as_object() {
                for (k, v) in custom {
                    options.insert(k.clone(), v.clone());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_config_defaults() {
        let config = LlmConfig::default();
        assert_eq!(config.temperature, 0.7);
        assert_eq!(config.max_tokens, 2048);
        assert!(!config.thinking);
        assert!(!config.json_mode);
        assert!(config.options.is_none());
    }

    #[test]
    fn test_llm_config_builder() {
        let config = LlmConfig::default()
            .with_temperature(0.3)
            .with_max_tokens(4096)
            .with_thinking(true)
            .with_json_mode(true);
        assert_eq!(config.temperature, 0.3);
        assert_eq!(config.max_tokens, 4096);
        assert!(config.thinking);
        assert!(config.json_mode);
    }
}
