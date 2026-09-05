//! Chat streaming logic extracted from chat/mod.rs.
//!
//! Contains `stream_chat_response` and the provider smoke test.

use crate::commands::chat::types::*;
use crate::db::notebook_db::Message;
use crate::error::GlossError;
use crate::providers::{ChatMessage, ChatRequest, ChatToken, LlmExecutionContext, LlmProvider};
use crate::retrieval::context::ContextPassage;
use crate::retrieval::source_scope::ResolvedSourceScope;
use crate::state::AppState;
use futures::StreamExt;
use receipts::ChatAttemptTraceV1;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::emit::{emit_chat_status, emit_chat_token};
use super::receipts;

const CHAT_CANCELLED_NOTEBOOK_SWITCH: &str = "__chat_cancelled_notebook_switch__";
const CHAT_CANCELLED_USER_REQUEST: &str = "__chat_cancelled_user_request__";

pub(crate) fn digest_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub(crate) fn source_context_digest(source_context: &[ContextPassage]) -> String {
    let mut material = String::new();
    for passage in source_context {
        material.push_str(&passage.source_id);
        material.push('\n');
        material.push_str(passage.chunk_id.as_deref().unwrap_or(""));
        material.push('\n');
        material.push_str(&passage.evidence_class);
        material.push('\n');
        material.push_str(&digest_text(&passage.content));
        material.push('\n');
    }
    digest_text(&material)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn stream_chat_response<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    provider: &dyn LlmProvider,
    notebook_id: &str,
    epoch: u64,
    conversation_id: &str,
    message_id: &str,
    query: &str,
    model: &str,
    history: &[Message],
    custom_goal: Option<&str>,
    style: &str,
    response_length: &str,
    resolved_scope: &ResolvedSourceScope,
    source_context: &[ContextPassage],
    model_context_window: Option<i32>,
    attempt_trace: &Arc<Mutex<ChatAttemptTraceV1>>,
    trace_data_dir: &Path,
    execution_context: LlmExecutionContext,
) -> Result<ChatStreamResult, GlossError> {
    use tauri::Manager;
    let state: tauri::State<'_, AppState> = app_handle.state();
    let provider_start_timeout = execution_context.timeouts.provider_start;
    let first_token_timeout = execution_context.timeouts.first_token;
    let stream_idle_timeout = execution_context.timeouts.stream_idle;

    // Build system prompt with source manifest and authority rules only.
    let system_prompt = crate::retrieval::context::ContextAssembler::build_system_prompt(
        custom_goal,
        style,
        resolved_scope.kind(),
        resolved_scope.manifest_sources(),
    );

    tracing::info!(
        system_prompt_len = system_prompt.len(),
        provider = provider.provider_type().as_str(),
        "System prompt built for LLM"
    );
    emit_chat_status(
        app_handle,
        notebook_id,
        conversation_id,
        message_id,
        "building_context",
        "Building prompt context",
        Some(provider.provider_type().as_str()),
        Some(model),
        None,
        None,
        None,
        Duration::ZERO,
        None,
        false,
        None,
    );

    // Build chat messages: history + user query
    let mut chat_messages: Vec<ChatMessage> = Vec::new();

    let history_msgs = crate::retrieval::context::ContextAssembler::format_history(history, 10);
    for (role, content) in &history_msgs {
        chat_messages.push(ChatMessage {
            role: role.clone(),
            content: content.clone(),
            images: None,
        });
    }

    let user_turn =
        crate::retrieval::context::ContextAssembler::build_user_turn(query, source_context);
    chat_messages.push(ChatMessage {
        role: "user".to_string(),
        content: user_turn,
        images: None,
    });

    let base_max_tokens: u32 = 2048;
    let response_length_multiplier: f64 = match response_length {
        "short" => 0.5,
        "long" => 2.0,
        _ => 1.0,
    };
    let max_tokens = (base_max_tokens as f64 * response_length_multiplier)
        .round()
        .max(1.0) as u32;
    let decoding_settings_receipt =
        super::effective_decoding_settings(&state, provider.provider_type(), model, max_tokens)?;
    let num_ctx_result = super::compute_dynamic_num_ctx(
        &system_prompt,
        &chat_messages,
        model_context_window,
        max_tokens,
    );
    let num_ctx = num_ctx_result.num_ctx;
    let request_material = serde_json::json!({
        "system": &system_prompt,
        "messages": &chat_messages,
        "model": model,
        "num_ctx": num_ctx,
        "max_tokens": max_tokens,
        "decoding_settings": &decoding_settings_receipt.effective,
    })
    .to_string();
    let request_digest = digest_text(&request_material);
    let user_turn_digest = chat_messages
        .last()
        .map(|message| digest_text(&message.content))
        .unwrap_or_else(|| digest_text(""));
    let context_payload_digest = source_context_digest(source_context);
    let prompt_receipt = PromptReceiptV1 {
        schema: "PromptReceiptV1".to_string(),
        receipt_id: uuid::Uuid::new_v4().to_string(),
        notebook_id: notebook_id.to_string(),
        conversation_id: conversation_id.to_string(),
        message_id: message_id.to_string(),
        prompt_digest: request_digest.clone(),
        context_payload_digest,
        capture_state: "captured_system_prompt".to_string(),
        redaction_state: "system_prompt_stored_other_content_digest_only".to_string(),
        system_prompt_digest: digest_text(&system_prompt),
        system_prompt_text: Some(system_prompt.clone()),
        user_turn_digest,
        source_passage_count: source_context.len(),
        recorded_at: chrono::Utc::now().to_rfc3339(),
    };
    let prompt_budget_receipt = PromptBudgetReceiptV1 {
        model_context_window: num_ctx,
        system_prompt_chars: request_material.len(),
        message_count: history_msgs.len() + 1,
        source_passage_count: source_context.len(),
        prompt_digest: request_digest.clone(),
        context_budgeted: num_ctx_result.context_budgeted,
        estimated_prompt_tokens: num_ctx_result.prompt_tokens,
    };
    let prompt_budget_detail = serde_json::to_string(&prompt_budget_receipt).ok();

    // Emit context-budgeted disclosure when prompt exceeds model context window
    if num_ctx_result.context_budgeted {
        emit_chat_status(
            app_handle,
            notebook_id,
            conversation_id,
            message_id,
            "context_budgeted",
            &format!(
                "Prompt (~{} tokens) exceeds model context window ({} tokens). Context was budgeted to fit.",
                num_ctx_result.needed, num_ctx
            ),
            Some(provider.provider_type().as_str()),
            Some(model),
            None,
            None,
            None,
            Duration::ZERO,
            None,
            true,
            None,
        );
    }

    receipts::record_chat_attempt_trace(
        attempt_trace,
        trace_data_dir,
        "prompt_budget_receipt",
        Some(Duration::ZERO),
        prompt_budget_detail.as_deref(),
        None,
        |_| {},
    );

    // Build the provider-agnostic chat request
    let request = ChatRequest {
        model: model.to_string(),
        system_prompt: Some(system_prompt),
        messages: chat_messages,
        max_tokens,
        temperature: decoding_settings_receipt.effective.temperature,
        top_p: decoding_settings_receipt.effective.top_p,
        top_k: decoding_settings_receipt.effective.top_k,
        min_p: decoding_settings_receipt.effective.min_p,
        repeat_penalty: decoding_settings_receipt.effective.repeat_penalty,
        stream: true,
        num_ctx: Some(num_ctx),
    };

    if !state.is_active_notebook_epoch(notebook_id, epoch) {
        receipts::record_chat_attempt_trace(
            attempt_trace,
            trace_data_dir,
            "cancelled",
            Some(Duration::ZERO),
            Some("Active notebook epoch changed before provider request"),
            Some(CHAT_CANCELLED_NOTEBOOK_SWITCH),
            |_| {},
        );
        return Err(GlossError::Other(CHAT_CANCELLED_NOTEBOOK_SWITCH.into()));
    }

    // Call the provider, but keep checking notebook epoch while waiting for the
    // first response so a switch can cancel the HTTP request promptly.
    let started = Instant::now();
    emit_chat_status(
        app_handle,
        notebook_id,
        conversation_id,
        message_id,
        "provider_request_start",
        "Starting provider request",
        Some(provider.provider_type().as_str()),
        Some(model),
        None,
        None,
        None,
        started.elapsed(),
        Some(provider_start_timeout),
        false,
        None,
    );
    receipts::record_chat_attempt_trace(
        attempt_trace,
        trace_data_dir,
        "provider_request_start",
        Some(started.elapsed()),
        Some("Starting provider request"),
        None,
        |_| {},
    );
    let chat_future = provider.chat(request, execution_context.clone());
    tokio::pin!(chat_future);
    let mut token_stream = loop {
        if execution_context.is_cancelled() {
            receipts::record_chat_attempt_trace(
                attempt_trace,
                trace_data_dir,
                "cancelled",
                Some(started.elapsed()),
                Some("Chat cancelled by user before provider start completed"),
                Some(CHAT_CANCELLED_USER_REQUEST),
                |_| {},
            );
            return Err(GlossError::Other(CHAT_CANCELLED_USER_REQUEST.into()));
        }
        if !state.is_active_notebook_epoch(notebook_id, epoch) {
            execution_context.cancellation.cancel();
            receipts::record_chat_attempt_trace(
                attempt_trace,
                trace_data_dir,
                "cancelled",
                Some(started.elapsed()),
                Some("Active notebook epoch changed during provider start"),
                Some(CHAT_CANCELLED_NOTEBOOK_SWITCH),
                |_| {},
            );
            return Err(GlossError::Other(CHAT_CANCELLED_NOTEBOOK_SWITCH.into()));
        }
        if started.elapsed() >= provider_start_timeout {
            execution_context.cancellation.cancel();
            let error = "Provider did not start streaming before the provider-start timeout";
            emit_chat_status(
                app_handle,
                notebook_id,
                conversation_id,
                message_id,
                "provider_start_timeout",
                error,
                Some(provider.provider_type().as_str()),
                Some(model),
                None,
                None,
                None,
                started.elapsed(),
                Some(provider_start_timeout),
                false,
                Some(error),
            );
            receipts::record_chat_attempt_trace(
                attempt_trace,
                trace_data_dir,
                "provider_start_timeout",
                Some(started.elapsed()),
                Some("Provider did not return a stream before timeout"),
                Some(error),
                |_| {},
            );
            return Err(GlossError::Provider {
                provider: provider.provider_type().as_str().to_string(),
                source: anyhow::anyhow!(error),
            });
        }

        tokio::select! {
            _ = execution_context.cancellation.cancelled() => {
                receipts::record_chat_attempt_trace(
                    attempt_trace,
                    trace_data_dir,
                    "cancelled",
                    Some(started.elapsed()),
                    Some("Chat cancelled by user while waiting for provider start"),
                    Some(CHAT_CANCELLED_USER_REQUEST),
                    |_| {},
                );
                return Err(GlossError::Other(CHAT_CANCELLED_USER_REQUEST.into()));
            }
            result = &mut chat_future => match result {
                Ok(stream) => break stream,
                Err(err) => {
                    let error = err.to_string();
                    let invocation = LlmInvocationReceiptV1 {
                        provider: provider.provider_type().as_str().to_string(),
                        model: model.to_string(),
                        request_digest: request_digest.clone(),
                        response_digest: None,
                        error: Some(error.clone()),
                    };
                    let invocation_detail = serde_json::to_string(&invocation).ok();
                    receipts::record_chat_attempt_trace(
                        attempt_trace,
                        trace_data_dir,
                        "llm_invocation_receipt",
                        Some(started.elapsed()),
                        invocation_detail.as_deref(),
                        Some(&error),
                        |_| {},
                    );
                    return Err(err);
                }
            },
            _ = tokio::time::sleep(Duration::from_millis(250)) => {}
        }
    };
    let first_token_wait_started = Instant::now();
    emit_chat_status(
        app_handle,
        notebook_id,
        conversation_id,
        message_id,
        "first_token_wait",
        "Waiting for first token",
        Some(provider.provider_type().as_str()),
        Some(model),
        None,
        None,
        None,
        first_token_wait_started.elapsed(),
        Some(first_token_timeout),
        false,
        None,
    );
    receipts::record_chat_attempt_trace(
        attempt_trace,
        trace_data_dir,
        "first_token_wait",
        Some(first_token_wait_started.elapsed()),
        Some("Waiting for first provider token"),
        None,
        |_| {},
    );

    let mut full_response = String::new();
    let mut done_frame_seen = false;
    let mut eof_seen = false;
    let mut first_token_seen = false;
    let mut last_token_at = Instant::now();
    let mut chunks_seen = 0usize;
    let mut terminal_cause: Option<&'static str> = None;

    loop {
        if execution_context.is_cancelled() {
            receipts::record_chat_attempt_trace(
                attempt_trace,
                trace_data_dir,
                "cancelled",
                Some(started.elapsed()),
                Some("Chat cancelled by user during provider stream"),
                Some(CHAT_CANCELLED_USER_REQUEST),
                |_| {},
            );
            return Err(GlossError::Other(CHAT_CANCELLED_USER_REQUEST.into()));
        }
        if !state.is_active_notebook_epoch(notebook_id, epoch) {
            execution_context.cancellation.cancel();
            receipts::record_chat_attempt_trace(
                attempt_trace,
                trace_data_dir,
                "cancelled",
                Some(started.elapsed()),
                Some("Active notebook epoch changed during provider stream"),
                Some(CHAT_CANCELLED_NOTEBOOK_SWITCH),
                |_| {},
            );
            return Err(GlossError::Other(CHAT_CANCELLED_NOTEBOOK_SWITCH.into()));
        }

        let next = tokio::select! {
            _ = execution_context.cancellation.cancelled() => {
                receipts::record_chat_attempt_trace(
                    attempt_trace,
                    trace_data_dir,
                    "cancelled",
                    Some(started.elapsed()),
                    Some("Chat cancelled by user while waiting for provider token"),
                    Some(CHAT_CANCELLED_USER_REQUEST),
                    |_| {},
                );
                return Err(GlossError::Other(CHAT_CANCELLED_USER_REQUEST.into()));
            }
            next = tokio::time::timeout(Duration::from_millis(250), token_stream.next()) => next
        };
        let next = match next {
            Ok(next) => next,
            Err(_) => {
                let timeout = if first_token_seen {
                    stream_idle_timeout
                } else {
                    first_token_timeout
                };
                let elapsed = if first_token_seen {
                    last_token_at.elapsed()
                } else {
                    first_token_wait_started.elapsed()
                };
                if elapsed >= timeout {
                    execution_context.cancellation.cancel();
                    let phase = if first_token_seen {
                        "stream_idle_timeout"
                    } else {
                        "first_token_timeout"
                    };
                    let error = if first_token_seen {
                        "Provider stream was idle past the stream-idle timeout"
                    } else {
                        "Provider did not produce a first token before timeout"
                    };
                    emit_chat_status(
                        app_handle,
                        notebook_id,
                        conversation_id,
                        message_id,
                        phase,
                        error,
                        Some(provider.provider_type().as_str()),
                        Some(model),
                        None,
                        None,
                        None,
                        elapsed,
                        Some(timeout),
                        first_token_seen,
                        Some(error),
                    );
                    receipts::record_chat_attempt_trace(
                        attempt_trace,
                        trace_data_dir,
                        phase,
                        Some(elapsed),
                        Some(error),
                        Some(error),
                        |_| {},
                    );
                    return Err(GlossError::Provider {
                        provider: provider.provider_type().as_str().to_string(),
                        source: anyhow::anyhow!(error),
                    });
                }
                continue;
            }
        };
        let Some(result) = next else {
            eof_seen = true;
            break;
        };

        if execution_context.is_cancelled() {
            receipts::record_chat_attempt_trace(
                attempt_trace,
                trace_data_dir,
                "late_chunk_after_cancel",
                Some(started.elapsed()),
                Some("Provider yielded a chunk after cancellation; chunk was ignored"),
                Some(CHAT_CANCELLED_USER_REQUEST),
                |_| {},
            );
            return Err(GlossError::Other(CHAT_CANCELLED_USER_REQUEST.into()));
        }

        let ChatToken { token, done } = match result {
            Ok(token) => token,
            Err(err) => {
                if execution_context.is_cancelled() {
                    receipts::record_chat_attempt_trace(
                        attempt_trace,
                        trace_data_dir,
                        "cancelled",
                        Some(started.elapsed()),
                        Some("Provider stream observed cancellation"),
                        Some(CHAT_CANCELLED_USER_REQUEST),
                        |_| {},
                    );
                    return Err(GlossError::Other(CHAT_CANCELLED_USER_REQUEST.into()));
                }
                let error = err.to_string();
                let invocation = LlmInvocationReceiptV1 {
                    provider: provider.provider_type().as_str().to_string(),
                    model: model.to_string(),
                    request_digest: request_digest.clone(),
                    response_digest: None,
                    error: Some(error.clone()),
                };
                let invocation_detail = serde_json::to_string(&invocation).ok();
                receipts::record_chat_attempt_trace(
                    attempt_trace,
                    trace_data_dir,
                    "llm_invocation_receipt",
                    Some(started.elapsed()),
                    invocation_detail.as_deref(),
                    Some(&error),
                    |_| {},
                );
                return Err(err);
            }
        };
        if !first_token_seen {
            first_token_seen = true;
            emit_chat_status(
                app_handle,
                notebook_id,
                conversation_id,
                message_id,
                "streaming",
                "Streaming response",
                Some(provider.provider_type().as_str()),
                Some(model),
                None,
                None,
                None,
                started.elapsed(),
                None,
                false,
                None,
            );
            receipts::record_chat_attempt_trace(
                attempt_trace,
                trace_data_dir,
                "streaming",
                Some(started.elapsed()),
                Some("First provider token received"),
                None,
                |trace| {
                    trace.first_token_seen = true;
                },
            );
        }
        if !token.is_empty() {
            last_token_at = Instant::now();
            chunks_seen += 1;
        }

        full_response.push_str(&token);

        if done {
            let decision = provider_done_terminal_decision();
            done_frame_seen = decision.done_frame_seen;
            eof_seen = decision.eof_seen;
            terminal_cause = Some(decision.terminal_cause);
            receipts::record_chat_attempt_trace(
                attempt_trace,
                trace_data_dir,
                "provider_done_frame",
                Some(started.elapsed()),
                Some("Provider emitted done=true; terminalizing without waiting for HTTP EOF"),
                None,
                |trace| {
                    trace.done_seen = true;
                },
            );
        }

        emit_chat_token(
            app_handle,
            notebook_id,
            conversation_id,
            message_id,
            &token,
            done && provider_done_terminal_decision().emit_done_on_current_token,
        );

        if done_frame_seen && provider_done_terminal_decision().break_stream_loop {
            break;
        }
    }

    if !done_frame_seen {
        let error = "Provider stream ended before a clean completion; response is incomplete";
        emit_chat_status(
            app_handle,
            notebook_id,
            conversation_id,
            message_id,
            "incomplete_stream",
            error,
            Some(provider.provider_type().as_str()),
            Some(model),
            None,
            None,
            None,
            started.elapsed(),
            None,
            true,
            Some(error),
        );
        receipts::record_chat_attempt_trace(
            attempt_trace,
            trace_data_dir,
            "incomplete_stream",
            Some(started.elapsed()),
            Some(error),
            Some(error),
            |_| {},
        );
        return Err(GlossError::Provider {
            provider: provider.provider_type().as_str().to_string(),
            source: anyhow::anyhow!(error),
        });
    }

    if full_response.trim().is_empty() {
        let error = "Provider stream completed without response content";
        emit_chat_status(
            app_handle,
            notebook_id,
            conversation_id,
            message_id,
            "empty_response",
            error,
            Some(provider.provider_type().as_str()),
            Some(model),
            None,
            None,
            None,
            started.elapsed(),
            None,
            false,
            Some(error),
        );
        receipts::record_chat_attempt_trace(
            attempt_trace,
            trace_data_dir,
            "empty_response",
            Some(started.elapsed()),
            Some(error),
            Some(error),
            |_| {},
        );
        return Err(GlossError::Provider {
            provider: provider.provider_type().as_str().to_string(),
            source: anyhow::anyhow!(error),
        });
    }

    emit_chat_status(
        app_handle,
        notebook_id,
        conversation_id,
        message_id,
        "complete",
        "Response complete",
        Some(provider.provider_type().as_str()),
        Some(model),
        None,
        None,
        None,
        started.elapsed(),
        None,
        false,
        None,
    );
    receipts::record_chat_attempt_trace(
        attempt_trace,
        trace_data_dir,
        "complete",
        Some(started.elapsed()),
        Some("Provider done frame completed generation"),
        None,
        |_| {},
    );
    let invocation = LlmInvocationReceiptV1 {
        provider: provider.provider_type().as_str().to_string(),
        model: model.to_string(),
        request_digest: request_digest.clone(),
        response_digest: Some(digest_text(&full_response)),
        error: None,
    };
    let invocation_detail = serde_json::to_string(&invocation).ok();
    receipts::record_chat_attempt_trace(
        attempt_trace,
        trace_data_dir,
        "llm_invocation_receipt",
        Some(started.elapsed()),
        invocation_detail.as_deref(),
        None,
        |_| {},
    );

    tracing::debug!(
        message_id,
        done_frame_seen,
        eof_seen,
        len = full_response.len(),
        "Chat response complete"
    );

    let generation_receipt = GenerationReceiptV1 {
        schema: "GenerationReceiptV1".to_string(),
        receipt_id: uuid::Uuid::new_v4().to_string(),
        notebook_id: notebook_id.to_string(),
        conversation_id: conversation_id.to_string(),
        message_id: message_id.to_string(),
        provider: provider.provider_type().as_str().to_string(),
        model: model.to_string(),
        provider_request_digest: request_digest,
        response_digest: Some(digest_text(&full_response)),
        status: "completed".to_string(),
        error: None,
        terminal_cause: terminal_cause.map(str::to_string),
        done_frame_seen,
        eof_seen,
        partial_persisted: true,
        chunks_seen,
        prompt_receipt_id: prompt_receipt.receipt_id.clone(),
        decoding_settings_receipt_id: decoding_settings_receipt.receipt_id.clone(),
        recorded_at: chrono::Utc::now().to_rfc3339(),
    };

    Ok(ChatStreamResult {
        full_response,
        decoding_settings_receipt,
        prompt_receipt,
        generation_receipt,
        prompt_budget_receipt: Some(prompt_budget_receipt),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::chat::receipts::new_chat_attempt_trace;
    use crate::retrieval::source_scope::SourceScope;
    use async_trait::async_trait;
    use futures::stream;
    use tauri::Manager;
    use tempfile::tempdir;

    enum ScriptedFailureMode {
        ProviderStartNeverReturns,
        FirstTokenNeverArrives,
        IdleAfterFirstToken,
        ProviderReturnsError,
        CancelDuringStream,
    }

    struct ScriptedFailureProvider(ScriptedFailureMode);

    #[async_trait]
    impl LlmProvider for ScriptedFailureProvider {
        async fn list_models(&self) -> Result<Vec<crate::providers::ModelInfo>, GlossError> {
            Ok(Vec::new())
        }

        async fn chat(
            &self,
            _request: ChatRequest,
            _context: LlmExecutionContext,
        ) -> Result<
            std::pin::Pin<Box<dyn futures::Stream<Item = Result<ChatToken, GlossError>> + Send>>,
            GlossError,
        > {
            match self.0 {
                ScriptedFailureMode::ProviderStartNeverReturns => {
                    futures::future::pending::<
                        Result<
                            std::pin::Pin<
                                Box<
                                    dyn futures::Stream<Item = Result<ChatToken, GlossError>>
                                        + Send,
                                >,
                            >,
                            GlossError,
                        >,
                    >()
                    .await
                }
                ScriptedFailureMode::FirstTokenNeverArrives => Ok(Box::pin(stream::pending())),
                ScriptedFailureMode::IdleAfterFirstToken => Ok(Box::pin(
                    stream::iter([Ok(ChatToken {
                        token: "partial".to_string(),
                        done: false,
                    })])
                    .chain(stream::pending()),
                )),
                ScriptedFailureMode::ProviderReturnsError => {
                    Err(GlossError::Other("scripted provider failure".to_string()))
                }
                ScriptedFailureMode::CancelDuringStream => {
                    let cancellation = _context.cancellation.clone();
                    let stream = stream::unfold(0_u8, move |step| {
                        let cancellation = cancellation.clone();
                        async move {
                            match step {
                                0 => Some((
                                    Ok(ChatToken {
                                        token: "partial".to_string(),
                                        done: false,
                                    }),
                                    1,
                                )),
                                1 => {
                                    cancellation.cancel();
                                    futures::future::pending::<
                                        Option<(Result<ChatToken, GlossError>, u8)>,
                                    >()
                                    .await
                                }
                                _ => None,
                            }
                        }
                    });
                    Ok(Box::pin(stream))
                }
            }
        }

        async fn health_check(&self) -> Result<bool, GlossError> {
            Ok(true)
        }

        fn provider_type(&self) -> crate::providers::ProviderType {
            crate::providers::ProviderType::Ollama
        }
    }

    async fn run_scripted_failure(
        mode: ScriptedFailureMode,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> (
        Result<ChatStreamResult, GlossError>,
        Vec<ChatStreamEventV1>,
        ChatAttemptTraceV1,
    ) {
        // Only the phase under test gets a short deadline. A one-millisecond
        // provider-start budget can expire during synchronous trace writes
        // before an immediate provider is polled, masking later-phase cases.
        let mut timeouts = crate::providers::LlmPhaseTimeouts {
            provider_start: Duration::from_secs(5),
            first_token: Duration::from_secs(5),
            stream_idle: Duration::from_secs(5),
        };
        match &mode {
            ScriptedFailureMode::ProviderStartNeverReturns => {
                timeouts.provider_start = Duration::from_millis(1);
            }
            ScriptedFailureMode::FirstTokenNeverArrives => {
                timeouts.first_token = Duration::from_millis(1);
            }
            ScriptedFailureMode::IdleAfterFirstToken => {
                timeouts.stream_idle = Duration::from_millis(1);
            }
            ScriptedFailureMode::ProviderReturnsError | ScriptedFailureMode::CancelDuringStream => {
            }
        }
        let data_dir = tempdir().expect("temporary state directory");
        let state = AppState::initialize_for_test(data_dir.path()).expect("test app state");
        state.set_active_notebook(Some("notebook-1".to_string()), Some(7));
        let app = tauri::test::mock_app();
        app.manage(state);
        let trace = Arc::new(Mutex::new(new_chat_attempt_trace(
            "notebook-1",
            "conversation-1",
            "message-1",
            "scripted-model",
            None,
            None,
            Some("none".to_string()),
        )));
        let scope = SourceScope::Explicit(Vec::new()).resolve(&[]);
        let result = stream_chat_response(
            app.handle(),
            &ScriptedFailureProvider(mode),
            "notebook-1",
            7,
            "conversation-1",
            "message-1",
            "hello",
            "scripted-model",
            &[],
            None,
            "balanced",
            "normal",
            &scope,
            &[],
            None,
            &trace,
            data_dir.path(),
            LlmExecutionContext::new(cancellation, timeouts),
        )
        .await;
        let events =
            app.state::<AppState>()
                .chat_events_since("notebook-1", "conversation-1", None);
        (
            result,
            events,
            receipts::chat_attempt_trace_snapshot(&trace),
        )
    }

    struct ScriptedDoneProvider;

    #[async_trait]
    impl LlmProvider for ScriptedDoneProvider {
        async fn list_models(&self) -> Result<Vec<crate::providers::ModelInfo>, GlossError> {
            Ok(Vec::new())
        }

        async fn chat(
            &self,
            _request: ChatRequest,
            _context: LlmExecutionContext,
        ) -> Result<
            std::pin::Pin<Box<dyn futures::Stream<Item = Result<ChatToken, GlossError>> + Send>>,
            GlossError,
        > {
            let stream = stream::unfold(0_u8, |step| async move {
                let token = match step {
                    0 => ChatToken {
                        token: "hello".to_string(),
                        done: false,
                    },
                    1 => ChatToken {
                        token: " world".to_string(),
                        done: false,
                    },
                    2 => ChatToken {
                        token: String::new(),
                        done: true,
                    },
                    _ => {
                        futures::future::pending::<()>().await;
                        unreachable!("pending fake transport cannot finish")
                    }
                };
                Some((Ok(token), step.saturating_add(1)))
            });
            Ok(Box::pin(stream))
        }

        async fn health_check(&self) -> Result<bool, GlossError> {
            Ok(true)
        }

        fn provider_type(&self) -> crate::providers::ProviderType {
            crate::providers::ProviderType::Ollama
        }
    }

    #[tokio::test]
    async fn scripted_done_frame_without_eof_completes_real_stream_path() {
        let data_dir = tempdir().expect("temporary state directory");
        let state = AppState::initialize_for_test(data_dir.path()).expect("test app state");
        state.set_active_notebook(Some("notebook-1".to_string()), Some(7));
        let app = tauri::test::mock_app();
        app.manage(state);

        let trace = Arc::new(Mutex::new(new_chat_attempt_trace(
            "notebook-1",
            "conversation-1",
            "message-1",
            "scripted-model",
            None,
            None,
            Some("none".to_string()),
        )));
        let scope = SourceScope::Explicit(Vec::new()).resolve(&[]);
        let provider = ScriptedDoneProvider;

        let result = stream_chat_response(
            app.handle(),
            &provider,
            "notebook-1",
            7,
            "conversation-1",
            "message-1",
            "hello",
            "scripted-model",
            &[],
            None,
            "balanced",
            "normal",
            &scope,
            &[],
            None,
            &trace,
            data_dir.path(),
            LlmExecutionContext::uncancellable(),
        )
        .await
        .expect("done frame must complete without waiting for EOF");

        assert_eq!(result.full_response, "hello world");
        assert_eq!(result.generation_receipt.status, "completed");
        assert_eq!(
            result.prompt_receipt.capture_state,
            "captured_system_prompt"
        );
        assert_eq!(
            result.prompt_receipt.redaction_state,
            "system_prompt_stored_other_content_digest_only"
        );
        let captured_prompt = result.prompt_receipt.system_prompt_text.as_ref().unwrap();
        assert!(!captured_prompt.is_empty());
        assert_eq!(
            result.prompt_receipt.system_prompt_digest,
            digest_text(captured_prompt)
        );
        assert!(result.generation_receipt.done_frame_seen);
        assert!(!result.generation_receipt.eof_seen);
        assert_eq!(
            app.state::<AppState>()
                .chat_events_since("notebook-1", "conversation-1", None)
                .iter()
                .filter(|event| event.kind == "token")
                .count(),
            3
        );
    }

    struct HistoryCapturingProvider {
        captured: Arc<Mutex<Vec<(String, String)>>>,
    }

    #[async_trait]
    impl LlmProvider for HistoryCapturingProvider {
        async fn list_models(&self) -> Result<Vec<crate::providers::ModelInfo>, GlossError> {
            Ok(Vec::new())
        }

        async fn chat(
            &self,
            request: ChatRequest,
            context: LlmExecutionContext,
        ) -> Result<
            std::pin::Pin<Box<dyn futures::Stream<Item = Result<ChatToken, GlossError>> + Send>>,
            GlossError,
        > {
            *self.captured.lock().unwrap() = request
                .messages
                .iter()
                .map(|message| (message.role.clone(), message.content.clone()))
                .collect();
            ScriptedDoneProvider.chat(request, context).await
        }

        async fn health_check(&self) -> Result<bool, GlossError> {
            Ok(true)
        }

        fn provider_type(&self) -> crate::providers::ProviderType {
            crate::providers::ProviderType::Ollama
        }
    }

    #[tokio::test]
    async fn edited_turn_sends_retained_history_then_replacement_query_exactly_once() {
        let data_dir = tempdir().unwrap();
        let state = AppState::initialize_for_test(data_dir.path()).unwrap();
        state.set_active_notebook(Some("notebook-1".to_string()), Some(7));
        let app = tauri::test::mock_app();
        app.manage(state);
        let trace = Arc::new(Mutex::new(new_chat_attempt_trace(
            "notebook-1",
            "conversation-1",
            "message-1",
            "scripted-model",
            None,
            None,
            Some("none".to_string()),
        )));
        let make_message = |id: &str, role: &str, content: &str| Message {
            id: id.to_string(),
            conversation_id: "conversation-1".to_string(),
            role: role.to_string(),
            content: content.to_string(),
            citations: None,
            model_used: None,
            tokens_prompt: None,
            tokens_response: None,
            created_at: String::new(),
        };
        let saved = vec![
            make_message("greeting", "user", "HELLO_GLOSS"),
            make_message("answer", "assistant", "HELLO_GLOSS"),
            make_message("cancelled", "user", "Write 100 ocean facts"),
            make_message("later", "user", "Later question"),
        ];
        let history =
            super::super::history_before_rerun(saved.clone(), "conversation-1", Some("cancelled"))
                .unwrap();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let provider = HistoryCapturingProvider {
            captured: Arc::clone(&captured),
        };
        let query = "Reply with exactly RETRY_GLOSS. /no_think";
        let scope = SourceScope::Explicit(Vec::new()).resolve(&[]);
        let result = stream_chat_response(
            app.handle(),
            &provider,
            "notebook-1",
            7,
            "conversation-1",
            "message-1",
            query,
            "scripted-model",
            &history,
            None,
            "balanced",
            "normal",
            &scope,
            &[],
            None,
            &trace,
            data_dir.path(),
            LlmExecutionContext::uncancellable(),
        )
        .await
        .unwrap();
        assert_eq!(
            *captured.lock().unwrap(),
            vec![
                ("user".to_string(), "HELLO_GLOSS".to_string()),
                ("assistant".to_string(), "HELLO_GLOSS".to_string()),
                ("user".to_string(), query.to_string()),
            ]
        );
        assert_eq!(result.prompt_receipt.user_turn_digest, digest_text(query));
        assert_eq!(saved.len(), 4);
        assert_eq!(saved[2].content, "Write 100 ocean facts");
    }

    #[tokio::test]
    async fn scripted_provider_start_timeout_records_its_typed_phase() {
        let (result, events, trace) = run_scripted_failure(
            ScriptedFailureMode::ProviderStartNeverReturns,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;

        assert!(result.is_err());
        assert!(events.iter().any(|event| {
            event.kind == "status" && event.payload["phase"] == "provider_start_timeout"
        }));
        assert!(trace
            .events
            .iter()
            .any(|event| event.phase == "provider_start_timeout"));
    }

    #[tokio::test]
    async fn scripted_first_token_timeout_records_its_typed_phase() {
        let (result, events, trace) = run_scripted_failure(
            ScriptedFailureMode::FirstTokenNeverArrives,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;

        assert!(result.is_err());
        assert!(events.iter().any(|event| {
            event.kind == "status" && event.payload["phase"] == "first_token_timeout"
        }));
        assert!(trace
            .events
            .iter()
            .any(|event| event.phase == "first_token_timeout"));
    }

    #[tokio::test]
    async fn scripted_stream_idle_timeout_records_its_typed_phase() {
        let (result, events, trace) = run_scripted_failure(
            ScriptedFailureMode::IdleAfterFirstToken,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;

        assert!(result.is_err());
        assert!(events.iter().any(|event| event.kind == "token"));
        assert!(events.iter().any(|event| {
            event.kind == "status" && event.payload["phase"] == "stream_idle_timeout"
        }));
        assert!(trace
            .events
            .iter()
            .any(|event| event.phase == "stream_idle_timeout"));
    }

    #[tokio::test]
    async fn scripted_provider_error_returns_provider_error_without_terminal_completion() {
        let (result, events, trace) = run_scripted_failure(
            ScriptedFailureMode::ProviderReturnsError,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;

        assert!(result.is_err());
        assert!(!events.iter().any(|event| event.kind == "done"));
        assert!(trace
            .events
            .iter()
            .any(|event| event.phase == "llm_invocation_receipt"));
    }

    #[tokio::test]
    async fn scripted_cancel_before_provider_start_is_not_reported_as_success() {
        let cancellation = tokio_util::sync::CancellationToken::new();
        cancellation.cancel();
        let (result, events, trace) =
            run_scripted_failure(ScriptedFailureMode::ProviderStartNeverReturns, cancellation)
                .await;

        assert!(result.is_err());
        assert!(!events.iter().any(|event| event.kind == "done"));
        assert!(trace.events.iter().any(|event| event.phase == "cancelled"));
    }

    #[tokio::test]
    async fn scripted_cancellation_during_stream_is_not_reported_as_success() {
        let (result, events, trace) = run_scripted_failure(
            ScriptedFailureMode::CancelDuringStream,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;

        assert!(result.is_err());
        assert!(events.iter().any(|event| event.kind == "token"));
        assert!(!events.iter().any(|event| event.kind == "done"));
        assert!(trace.events.iter().any(|event| event.phase == "cancelled"));
    }
}
