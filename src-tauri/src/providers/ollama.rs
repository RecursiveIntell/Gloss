use super::{
    provider_cancelled_error, provider_http_failure, ChatRequest, ChatToken, LlmExecutionContext,
    LlmProvider, ModelInfo, ProviderType,
};
use crate::error::GlossError;
use async_trait::async_trait;
use futures::stream::{self, Stream};
use futures::StreamExt;
use std::collections::VecDeque;
use std::pin::Pin;

const MAX_OLLAMA_FRAME_BYTES: usize = 1024 * 1024;
const MAX_OLLAMA_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

fn ollama_protocol_error(message: impl std::fmt::Display) -> GlossError {
    GlossError::Provider {
        provider: "ollama".into(),
        source: anyhow::anyhow!("Ollama response protocol error: {message}"),
    }
}

/// Ollama emits NDJSON, not independently decodable network chunks. Retain raw
/// bytes until a complete line (or exact EOF JSON value) is available. Never
/// repair malformed JSON, replace invalid UTF-8, skip bad frames or invent done.
#[derive(Default)]
struct OllamaStreamDecoder {
    line: Vec<u8>,
    terminal: bool,
    failed: bool,
}

impl OllamaStreamDecoder {
    fn parse_line(&mut self) -> Result<Option<ChatToken>, GlossError> {
        let bytes = std::mem::take(&mut self.line);
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| ollama_protocol_error("invalid UTF-8 in NDJSON frame"))?;
        // Empty separator lines and CRLF are accepted; neither is a response.
        if text.trim().is_empty() {
            return Ok(None);
        }
        let value = serde_json::from_str(text)
            .map_err(|_| ollama_protocol_error("malformed or truncated NDJSON frame"))?;
        let token = ollama_chat_token_from_value(&value)?;
        self.terminal = token.done;
        Ok(Some(token))
    }

    fn push(&mut self, bytes: &[u8]) -> Vec<Result<ChatToken, GlossError>> {
        let mut tokens = Vec::new();
        for &byte in bytes {
            if self.terminal || self.failed {
                break;
            }
            if byte == b'\n' {
                match self.parse_line() {
                    Ok(Some(token)) => tokens.push(Ok(token)),
                    Ok(None) => {}
                    Err(error) => {
                        self.failed = true;
                        tokens.push(Err(error));
                    }
                }
            } else if self.line.len() == MAX_OLLAMA_FRAME_BYTES {
                self.failed = true;
                self.line.clear();
                tokens.push(Err(ollama_protocol_error(
                    "NDJSON frame exceeds 1 MiB limit",
                )));
            } else {
                self.line.push(byte);
            }
        }
        tokens
    }

    fn finish(&mut self) -> Vec<Result<ChatToken, GlossError>> {
        if self.terminal || self.failed {
            return Vec::new();
        }
        let mut tokens = Vec::new();
        match self.parse_line() {
            Ok(Some(token)) => tokens.push(Ok(token)),
            Ok(None) => {}
            Err(error) => {
                self.failed = true;
                tokens.push(Err(error));
                return tokens;
            }
        }
        if !self.terminal {
            self.failed = true;
            tokens.push(Err(ollama_protocol_error("stream ended before done: true")));
        }
        tokens
    }
}

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
    if let Some(error) = val.get("error") {
        let message = error
            .as_str()
            .ok_or_else(|| ollama_protocol_error("error field must be a string"))?;
        return Err(GlossError::Provider {
            provider: "ollama".into(),
            source: anyhow::anyhow!(
                "Ollama stream error: {}",
                super::sanitize_provider_error_body(message)
            ),
        });
    }

    let done = val
        .get("done")
        .and_then(|d| d.as_bool())
        .ok_or_else(|| ollama_protocol_error("frame must contain boolean done"))?;
    let token = match val.get("message") {
        Some(message) => message
            .get("content")
            .and_then(|c| c.as_str())
            .ok_or_else(|| ollama_protocol_error("message.content must be a string"))?
            .to_string(),
        None if done => String::new(),
        None => {
            return Err(ollama_protocol_error(
                "nonterminal frame is missing message",
            ))
        }
    };
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
        ctx: LlmExecutionContext,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatToken, GlossError>> + Send>>, GlossError> {
        ctx.check_cancelled("ollama", "before_request_build")?;
        let url = format!("{}/api/chat", self.base_url);
        let body = build_ollama_chat_body(&request);

        ctx.check_cancelled("ollama", "before_http_send")?;
        let send = self.client.post(&url).json(&body).send();
        let resp = tokio::select! {
            _ = ctx.cancellation.cancelled() => {
                return Err(provider_cancelled_error("ollama", "waiting_for_response_headers", ctx.attempt_id.as_deref()));
            }
            result = send => result.map_err(|e| GlossError::Provider {
                provider: "ollama".into(),
                source: e.into(),
            })?
        };

        if !resp.status().is_success() {
            return Err(provider_http_failure("ollama", resp, &ctx).await);
        }

        if request.stream {
            let byte_stream = resp.bytes_stream();
            // One token per poll keeps cancellation authoritative even when one
            // HTTP chunk contains many JSON frames. Error/done fuses the stream.
            let stream = stream::unfold(
                Some((
                    byte_stream,
                    OllamaStreamDecoder::default(),
                    VecDeque::<Result<ChatToken, GlossError>>::new(),
                    ctx,
                )),
                |state| async move {
                    let (mut byte_stream, mut decoder, mut pending, ctx) = state?;
                    loop {
                        if ctx.is_cancelled() {
                            return Some((
                                Err(provider_cancelled_error(
                                    "ollama",
                                    "before_yield_token",
                                    ctx.attempt_id.as_deref(),
                                )),
                                None,
                            ));
                        }
                        if let Some(token) = pending.pop_front() {
                            let finished = match &token {
                                Ok(token) => token.done,
                                Err(_) => true,
                            };
                            let next = if finished {
                                None
                            } else {
                                Some((byte_stream, decoder, pending, ctx))
                            };
                            return Some((token, next));
                        }
                        let next = tokio::select! {
                            _ = ctx.cancellation.cancelled() => {
                                return Some((Err(provider_cancelled_error(
                                    "ollama", "reading_stream_chunk", ctx.attempt_id.as_deref(),
                                )), None));
                            }
                            next = byte_stream.next() => next,
                        };
                        match next {
                            Some(Ok(bytes)) => pending.extend(decoder.push(&bytes)),
                            Some(Err(error)) => {
                                return Some((
                                    Err(GlossError::Provider {
                                        provider: "ollama".into(),
                                        source: error.into(),
                                    }),
                                    None,
                                ));
                            }
                            None => pending.extend(decoder.finish()),
                        }
                    }
                },
            );

            Ok(Box::pin(stream.fuse()))
        } else {
            // Bound non-stream responses while reading, before JSON allocation.
            let mut resp = resp;
            let mut bytes = Vec::new();
            loop {
                let next = tokio::select! {
                    _ = ctx.cancellation.cancelled() => {
                        return Err(provider_cancelled_error("ollama", "reading_non_stream_response", ctx.attempt_id.as_deref()));
                    }
                    result = resp.chunk() => result.map_err(|error| GlossError::Provider {
                        provider: "ollama".into(), source: error.into(),
                    })?
                };
                let Some(chunk) = next else { break };
                if chunk.len() > MAX_OLLAMA_RESPONSE_BYTES - bytes.len() {
                    return Err(ollama_protocol_error(
                        "non-stream response exceeds 8 MiB limit",
                    ));
                }
                bytes.extend_from_slice(&chunk);
            }
            let body: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|_| ollama_protocol_error("malformed non-stream JSON response"))?;
            ctx.check_cancelled("ollama", "before_terminal_frame")?;

            let token = ollama_chat_token_from_value(&body)?;
            if !token.done {
                return Err(ollama_protocol_error(
                    "non-stream response is missing done: true",
                ));
            }
            Ok(Box::pin(stream::iter(vec![Ok(token)])))
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

    #[test]
    fn ndjson_preserves_multibyte_content_at_every_chunk_boundary() {
        let wire = "{\"message\":{\"content\":\"你好 🦀\"},\"done\":false}\r\n{\"done\":true}\n";
        for split in 0..=wire.len() {
            let mut decoder = OllamaStreamDecoder::default();
            let mut tokens = decoder.push(&wire.as_bytes()[..split]);
            tokens.extend(decoder.push(&wire.as_bytes()[split..]));
            tokens.extend(decoder.finish());
            let tokens: Vec<_> = tokens.into_iter().collect::<Result<_, _>>().unwrap();
            assert_eq!(tokens.len(), 2);
            assert_eq!(tokens[0].token, "你好 🦀");
            assert!(!tokens[0].done);
            assert!(tokens[1].done);
        }
        let mut decoder = OllamaStreamDecoder::default();
        let tokens: Vec<_> = wire
            .as_bytes()
            .chunks(1)
            .flat_map(|bytes| decoder.push(bytes))
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(tokens[0].token, "你好 🦀");
        assert!(tokens[1].done);
    }

    #[test]
    fn ndjson_preserves_prior_tokens_and_fails_at_first_bad_frame() {
        for bad in [
            b"{garbage}\n".as_slice(),
            b"{\"message\":{\"content\":\"\xff\"},\"done\":false}\n",
            b"{\"message\":{\"content\":17},\"done\":false}\n",
            b"{\"message\":{\"content\":\"lost\"}}\n",
            b"{\"done\":\"true\"}\n",
            b"{\"error\":17}\n",
            b"{\"done\":false}\n",
            b"[]\n",
        ] {
            let mut decoder = OllamaStreamDecoder::default();
            let mut bytes = b"{\"message\":{\"content\":\"kept\"},\"done\":false}\n".to_vec();
            bytes.extend_from_slice(bad);
            bytes.extend_from_slice(b"{\"done\":true}\n");
            let mut tokens = decoder.push(&bytes).into_iter();
            assert_eq!(tokens.next().unwrap().unwrap().token, "kept");
            assert!(tokens.next().unwrap().is_err());
            assert!(tokens.next().is_none());
            assert!(decoder.finish().is_empty());
            assert!(decoder.push(b"{\"done\":true}\n").is_empty());
        }
    }

    #[test]
    fn ndjson_eof_requires_complete_json_and_real_terminal_marker() {
        for wire in [
            "",
            " \r\n",
            "{\"done\":tru",
            "{\"done\":true",
            "{\"message\":{\"content\":\"partial\"},\"done\":false}",
            "{\"message\":{\"content\":\"partial\"},\"done\":false}\n",
        ] {
            let mut decoder = OllamaStreamDecoder::default();
            let mut tokens = decoder.push(wire.as_bytes());
            tokens.extend(decoder.finish());
            assert!(tokens.iter().any(Result::is_err), "must reject {wire:?}");
            assert!(!tokens
                .iter()
                .any(|item| item.as_ref().is_ok_and(|token| token.done)));
        }
        let mut decoder = OllamaStreamDecoder::default();
        assert!(decoder.push(b"{\"done\":true}").is_empty());
        assert!(decoder.finish().remove(0).unwrap().done);
    }

    #[test]
    fn ndjson_bounds_frame_before_unbounded_allocation() {
        let mut decoder = OllamaStreamDecoder::default();
        assert!(decoder.push(&vec![b'x'; MAX_OLLAMA_FRAME_BYTES]).is_empty());
        let error = decoder.push(b"x").remove(0).unwrap_err();
        assert!(error.to_string().contains("1 MiB"));
        assert!(decoder.line.is_empty());
        assert!(decoder.push(&vec![b'x'; MAX_OLLAMA_FRAME_BYTES]).is_empty());
    }

    #[tokio::test]
    async fn actual_ollama_stream_reports_malformed_truncated_and_missing_done() {
        for body in [
            b"{invalid}\n".as_slice(),
            b"{\"done\":true",
            b"{\"message\":{\"content\":\"partial\"},\"done\":false}\n",
            b"{\"message\":{\"content\":\"\xff\"},\"done\":false}\n",
        ] {
            let (url, fixture) =
                super::super::test_http::respond("200 OK", body.to_vec(), "").await;
            let provider = OllamaProvider::new(&url, super::super::build_shared_client().unwrap());
            let mut stream = provider
                .chat(smoke_request(), LlmExecutionContext::uncancellable())
                .await
                .unwrap();
            let mut error = None;
            while let Some(item) =
                tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                    .await
                    .unwrap()
            {
                match item {
                    Ok(token) => assert!(!token.done),
                    Err(err) => {
                        error = Some(err);
                        break;
                    }
                }
            }
            assert!(error.is_some(), "bad wire response must fail");
            assert!(stream.next().await.is_none(), "error must fuse stream");
            assert!(
                stream.next().await.is_none(),
                "closed stream remains closed"
            );
            fixture.await.unwrap();
        }
    }

    #[tokio::test]
    async fn actual_ollama_stream_finishes_and_cancel_discards_buffered_tokens() {
        let body = "{\"message\":{\"content\":\"你好 🦀\"},\"done\":false}\n{\"message\":{\"content\":\"later\"},\"done\":false}\n{\"done\":true}\n";
        for cancel_after_first in [false, true] {
            let (url, fixture) =
                super::super::test_http::respond("200 OK", body.as_bytes().to_vec(), "").await;
            let cancellation = tokio_util::sync::CancellationToken::new();
            let ctx = LlmExecutionContext::default_with_token(cancellation.clone());
            let provider = OllamaProvider::new(&url, super::super::build_shared_client().unwrap());
            let mut stream = provider.chat(smoke_request(), ctx).await.unwrap();
            assert_eq!(stream.next().await.unwrap().unwrap().token, "你好 🦀");
            if cancel_after_first {
                cancellation.cancel();
                assert!(stream
                    .next()
                    .await
                    .unwrap()
                    .unwrap_err()
                    .to_string()
                    .contains("cancelled"));
            } else {
                assert_eq!(stream.next().await.unwrap().unwrap().token, "later");
                assert!(stream.next().await.unwrap().unwrap().done);
            }
            assert!(stream.next().await.is_none());
            assert!(stream.next().await.is_none());
            fixture.await.unwrap();
        }
    }

    #[tokio::test]
    async fn actual_ollama_non_stream_does_not_invent_completion() {
        for body in [
            "{\"message\":{\"content\":\"partial\"},\"done\":false}",
            "{\"message\":{\"content\":\"partial\"}}",
        ] {
            let (url, fixture) =
                super::super::test_http::respond("200 OK", body.as_bytes().to_vec(), "").await;
            let provider = OllamaProvider::new(&url, super::super::build_shared_client().unwrap());
            let mut request = smoke_request();
            request.stream = false;
            assert!(provider
                .chat(request, LlmExecutionContext::uncancellable())
                .await
                .is_err());
            fixture.await.unwrap();
        }
    }

    #[tokio::test]
    async fn actual_ollama_non_stream_bounds_response_allocation() {
        let (url, fixture) = super::super::test_http::respond(
            "200 OK",
            vec![b' '; MAX_OLLAMA_RESPONSE_BYTES + 1],
            "",
        )
        .await;
        let provider = OllamaProvider::new(&url, super::super::build_shared_client().unwrap());
        let mut request = smoke_request();
        request.stream = false;
        let error = match provider
            .chat(request, LlmExecutionContext::uncancellable())
            .await
        {
            Ok(_) => panic!("oversized non-stream response must fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("8 MiB"));
        fixture.await.unwrap();
    }

    #[tokio::test]
    async fn actual_ollama_terminal_frame_finishes_without_waiting_for_eof() {
        let (url, fixture) = super::super::test_http::hold_open(
            "200 OK",
            b"{\"message\":{\"content\":\"complete\"},\"done\":true}\n".to_vec(),
        )
        .await;
        let provider = OllamaProvider::new(&url, super::super::build_shared_client().unwrap());
        let mut stream = provider
            .chat(smoke_request(), LlmExecutionContext::uncancellable())
            .await
            .unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let token = stream.next().await.unwrap().unwrap();
            assert_eq!(token.token, "complete");
            assert!(token.done);
            assert!(stream.next().await.is_none());
        })
        .await;
        fixture.abort();
        result.expect("a done frame ends the stream without waiting for server EOF");
    }
}
