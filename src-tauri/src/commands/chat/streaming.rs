//! Chat streaming logic extracted from chat/mod.rs.
//!
//! Contains `stream_chat_response` and the provider smoke test.

use crate::commands::chat::types::*;
use crate::db::notebook_db::Message;
use crate::error::GlossError;
use crate::providers::{ChatMessage, ChatRequest, ChatToken, LlmProvider};
use crate::retrieval::context::ContextPassage;
use crate::retrieval::source_scope::ResolvedSourceScope;
use crate::state::AppState;
use futures::StreamExt;
use receipts::ChatAttemptTraceV1;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::Emitter;

use super::emit::emit_chat_status;
use super::receipts;

const CHAT_CANCELLED_NOTEBOOK_SWITCH: &str = "__chat_cancelled_notebook_switch__";
const CHAT_PROVIDER_START_TIMEOUT: Duration = Duration::from_secs(180);
const CHAT_FIRST_TOKEN_TIMEOUT: Duration = Duration::from_secs(168);
const CHAT_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(84);

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
pub(crate) async fn stream_chat_response(
    app_handle: &tauri::AppHandle,
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
    resolved_scope: &ResolvedSourceScope,
    source_context: &[ContextPassage],
    model_context_window: Option<i32>,
    attempt_trace: &Arc<Mutex<ChatAttemptTraceV1>>,
    trace_data_dir: &Path,
) -> Result<ChatStreamResult, GlossError> {
    use tauri::Manager;
    let state: tauri::State<'_, AppState> = app_handle.state();

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

    let max_tokens = 2048;
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
        capture_state: "captured_digest_only".to_string(),
        redaction_state: "content_not_stored_in_receipt".to_string(),
        system_prompt_digest: digest_text(&system_prompt),
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
        Some(CHAT_PROVIDER_START_TIMEOUT),
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
    let chat_future = provider.chat(request);
    tokio::pin!(chat_future);
    let mut token_stream = loop {
        if !state.is_active_notebook_epoch(notebook_id, epoch) {
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
        if started.elapsed() >= CHAT_PROVIDER_START_TIMEOUT {
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
                Some(CHAT_PROVIDER_START_TIMEOUT),
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
        Some(CHAT_FIRST_TOKEN_TIMEOUT),
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
        if !state.is_active_notebook_epoch(notebook_id, epoch) {
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

        let next = match tokio::time::timeout(Duration::from_millis(250), token_stream.next()).await
        {
            Ok(next) => next,
            Err(_) => {
                let timeout = if first_token_seen {
                    CHAT_STREAM_IDLE_TIMEOUT
                } else {
                    CHAT_FIRST_TOKEN_TIMEOUT
                };
                let elapsed = if first_token_seen {
                    last_token_at.elapsed()
                } else {
                    first_token_wait_started.elapsed()
                };
                if elapsed >= timeout {
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

        let ChatToken { token, done } = match result {
            Ok(token) => token,
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

        let _ = app_handle.emit(
            "chat:token",
            serde_json::json!({
                "notebook_id": notebook_id,
                "conversation_id": conversation_id,
                "message_id": message_id,
                "token": token,
                "done": done && provider_done_terminal_decision().emit_done_on_current_token,
            }),
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
