#[cfg(feature = "semantic-memory-backend")]
use crate::db::notebook_db::NotebookDb;
use crate::db::notebook_db::{ChatAttemptStatus, Chunk, Conversation, Message, Source};
use crate::error::GlossError;
use crate::features;
use crate::jobs;
#[cfg(feature = "semantic-memory-backend")]
use crate::memory::semantic_memory_adapter;
use crate::memory::MemorySearchRequest;
use crate::memory::{
    RetrievalCapabilityDecisionV1, RetrievalMode, RetrievalOutcome, RetrievalReasonCode,
};
#[cfg(feature = "semantic-memory-backend")]
use crate::memory::{RetrievalCoverage, RetrievalEngineStatus, RetrievalResult};
use crate::memory::{MEMORY_BACKEND_GLOSS_LOCAL, MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW};
use crate::providers::{self, ChatMessage, ChatRequest, ChatToken};
use crate::retrieval::citations;
use crate::retrieval::context::ContextPassage;
use crate::retrieval::hybrid_search;
use crate::retrieval::source_scope::{ResolvedSourceScope, SourceScope};
use crate::state::AppState;
use crate::studio::build_snippets;
use futures::{FutureExt, StreamExt};
use serde::Serialize;

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Manager, State};
use tauri_queue::QueueManager;

mod emit;
mod gates;
mod history;
pub(crate) mod receipts;
mod streaming;
mod types;

// Re-export helpers used by send_message / stream_chat_response and other
// functions that remain in this file.
use emit::{emit_chat_error, emit_chat_evidence, emit_chat_status, ChatTerminalEmitter};
use gates::acquire_gate_with_epoch;
use history::{history_before_rerun, resolve_user_message_id};
use receipts::{chat_attempt_trace_snapshot, new_chat_attempt_trace, record_chat_attempt_trace};

// Pull in types from the types submodule.
use types::*;
// Pull in streaming helpers.
use streaming::{source_context_digest, stream_chat_response};

#[allow(unused_imports)]
pub use receipts::{ChatAttemptTraceEvent, ChatAttemptTraceV1};
pub(crate) use types::ChatStreamEventV1;

const CHAT_CANCELLED_NOTEBOOK_SWITCH: &str = "__chat_cancelled_notebook_switch__";
const CHAT_CANCELLED_USER_REQUEST: &str = "__chat_cancelled_user_request__";
const CHAT_PROVIDER_START_TIMEOUT: Duration = Duration::from_secs(180);
const CHAT_FIRST_TOKEN_TIMEOUT: Duration = Duration::from_secs(168);
const CHAT_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(84);
const DEFAULT_CHAT_TEMPERATURE: f32 = 0.7;
#[cfg(feature = "semantic-memory-backend")]
const SEMANTIC_MEMORY_SEARCH_TIMEOUT: Duration = Duration::from_secs(8);
const DEFAULT_CONTEXT_WINDOW_TOKENS: u32 = 8_192;
const MAX_CONTEXT_WINDOW_TOKENS: u32 = 32_768;
/// Total character budget for retrieved passages injected into the user turn.
/// Keeps the prompt inside MAX_CONTEXT_WINDOW_TOKENS even at the largest top_k.
const MAX_CONTEXT_CHARS_TOTAL: usize = 32_000;

/// A degraded optional engine does not invalidate context from an available
/// local index. Keep this separate from the caller's ID, scope and link checks.
fn local_indexed_fallback_preserves_scope(
    requested_backend: &str,
    outcome: Option<&RetrievalOutcome>,
    context: &[ContextPassage],
) -> bool {
    if requested_backend != MEMORY_BACKEND_GLOSS_LOCAL || context.is_empty() {
        return false;
    }
    let Some(outcome) = outcome else {
        return false;
    };
    let contributed = |name: &str| {
        outcome.engines.iter().any(|engine| {
            engine.engine == name && engine.available && engine.attempted && engine.contributed
        })
    };
    let indexed = match outcome.mode {
        RetrievalMode::Bm25Only => contributed("bm25_fts5"),
        RetrievalMode::DenseOnly => contributed("native_dense_hnsw"),
        RetrievalMode::HybridRrf => contributed("bm25_fts5") && contributed("native_dense_hnsw"),
        _ => false,
    };
    indexed
        && context.iter().all(|passage| {
            passage.evidence_class == outcome.mode.as_str()
                && passage.chunk_id.is_some()
                && outcome.results.iter().any(|result| {
                    result.source_id == passage.source_id && result.chunk_id == passage.chunk_id
                })
        })
}

fn native_dense_evidence(outcome: Option<&RetrievalOutcome>) -> (bool, String) {
    match outcome.and_then(|outcome| {
        outcome
            .engines
            .iter()
            .find(|engine| engine.engine == "native_dense_hnsw")
    }) {
        Some(engine) if engine.available => (true, "native-dense-enabled".to_string()),
        Some(engine) => (
            false,
            engine
                .reason_code
                .as_ref()
                .map(|reason| reason.as_str())
                .unwrap_or("native-dense-unavailable")
                .to_string(),
        ),
        None => (false, "not-observed".to_string()),
    }
}

// All struct/type definitions moved to types.rs; imported via `use types::*` above.

/// Owns every value that the asynchronous chat lifecycle consumes after
/// `send_message` has persisted the user turn and assembled retrieval context.
///
/// Keeping this as a runtime-generic job makes the real lifecycle directly
/// callable under `tauri::test::mock_app` without changing the production
/// state or provider path.
struct SpawnedChatAttempt<R: tauri::Runtime> {
    handle: tauri::AppHandle<R>,
    active_chat_attempt: crate::state::ActiveChatAttempt,
    provider: Box<dyn providers::LlmProvider>,
    terminal: ChatTerminalEmitter<R>,
    notebook_id: String,
    conversation_id: String,
    message_id: String,
    query: String,
    model: String,
    history: Vec<Message>,
    custom_goal: Option<String>,
    style: String,
    response_length: String,
    source_scope: ResolvedSourceScope,
    source_context: Vec<ContextPassage>,
    model_context_window: Option<i32>,
    evidence_for_message: ChatEvidenceDisclosure,
    epoch: u64,
    attempt_id: String,
    user_message_id: String,
    provider_name: String,
    operation_receipt_id: String,
    attempt_trace: Arc<Mutex<ChatAttemptTraceV1>>,
    trace_data_dir: std::path::PathBuf,
    phase_timeouts: providers::LlmPhaseTimeouts,
}

fn panic_payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "non-string panic payload".to_string()
}

impl<R: tauri::Runtime> SpawnedChatAttempt<R> {
    /// Convert an unexpected task panic into the same durable terminal
    /// contract as provider/stream failures. Without this boundary, a panic
    /// would drop the detached JoinHandle and leave the frontend in a
    /// permanent streaming state.
    pub(crate) async fn run_with_panic_boundary(self) {
        let handle = self.handle.clone();
        let terminal = self.terminal.clone();
        let notebook_id = self.notebook_id.clone();
        let conversation_id = self.conversation_id.clone();
        let attempt_id = self.attempt_id.clone();
        let message_id = self.message_id.clone();
        let user_message_id = self.user_message_id.clone();
        let provider_name = self.provider_name.clone();
        let model = self.model.clone();
        let attempt_trace = Arc::clone(&self.attempt_trace);
        let trace_data_dir = self.trace_data_dir.clone();

        if let Err(payload) = std::panic::AssertUnwindSafe(self.run())
            .catch_unwind()
            .await
        {
            let error = format!(
                "Chat task panicked: {}",
                panic_payload_message(payload.as_ref())
            );
            let app_state: tauri::State<'_, AppState> = handle.state();
            tracing::error!(
                notebook_id = %notebook_id,
                conversation_id = %conversation_id,
                attempt_id = %attempt_id,
                message_id = %message_id,
                error = %error,
                "Detached chat task panicked"
            );
            if !terminal.is_fired() {
                record_chat_attempt_trace(
                    &attempt_trace,
                    &trace_data_dir,
                    "chat_task_panic",
                    Some(Duration::ZERO),
                    Some("Detached chat task panicked"),
                    Some(&error),
                    |_| {},
                );
                persist_chat_attempt_status(
                    &app_state,
                    &notebook_id,
                    &conversation_id,
                    &attempt_id,
                    &message_id,
                    Some(&user_message_id),
                    Some(&provider_name),
                    Some(&model),
                    "error",
                    Some("chat_task_panic"),
                    Some("chat_task_panic"),
                    Some(&error),
                    None,
                    true,
                );
                terminal.emit_error(&error);
            }
            app_state.finish_active_chat_attempt(&notebook_id, &conversation_id, &attempt_id);
        }
    }

    /// Runs the sole owner of the post-context chat lifecycle.
    pub(crate) async fn run(self) {
        use tauri::Manager;

        let Self {
            handle,
            active_chat_attempt,
            provider,
            terminal,
            notebook_id,
            conversation_id,
            message_id,
            query,
            model,
            history,
            custom_goal,
            style,
            response_length,
            source_scope,
            source_context,
            model_context_window,
            evidence_for_message,
            epoch,
            attempt_id,
            user_message_id,
            provider_name,
            operation_receipt_id,
            attempt_trace,
            trace_data_dir,
            phase_timeouts,
        } = self;
        let app_state: tauri::State<'_, AppState> = handle.state();
        let execution_context = providers::LlmExecutionContext::new(
            active_chat_attempt.cancellation.clone(),
            phase_timeouts,
        )
        .with_operation(providers::LlmOperationContext::chat(
            notebook_id.clone(),
            conversation_id.clone(),
            message_id.clone(),
            epoch,
            attempt_id.clone(),
            provider_name.clone(),
            model.clone(),
            operation_receipt_id.clone(),
        ));

        // Acquire gates in the same order as the background summary loop:
        // GPU first, then LLM. Keeping one global order prevents lock-order
        // inversion when chat races a summary job.
        let gpu_permit = match acquire_gate_with_epoch(
            &handle,
            &app_state,
            &notebook_id,
            &conversation_id,
            &message_id,
            epoch,
            &app_state.gpu_gate,
            Duration::from_secs(120),
            "GPU gate",
        )
        .await
        {
            Ok(Some(permit)) => permit,
            Ok(None) => {
                execution_context.cancellation.cancel();
                record_chat_attempt_trace(
                    &attempt_trace,
                    &trace_data_dir,
                    "cancelled",
                    Some(Duration::ZERO),
                    Some("GPU gate wait cancelled by notebook switch"),
                    Some("chat cancelled before acquiring GPU gate"),
                    |_| {},
                );
                persist_chat_attempt_status(
                    &app_state,
                    &notebook_id,
                    &conversation_id,
                    &attempt_id,
                    &message_id,
                    Some(&user_message_id),
                    Some(&provider_name),
                    Some(&model),
                    "cancelled",
                    Some("gpu_gate_cancelled"),
                    Some("cancelled"),
                    Some("chat cancelled before acquiring GPU gate"),
                    None,
                    true,
                );
                app_state.finish_active_chat_attempt(&notebook_id, &conversation_id, &attempt_id);
                terminal.emit_cancelled("Chat cancelled before acquiring GPU gate");
                return;
            }
            Err(e) => {
                tracing::error!(message_id = %message_id, error = %e, "GPU gate acquisition failed");
                terminal.emit_error(&e.to_string());
                persist_chat_attempt_status(
                    &app_state,
                    &notebook_id,
                    &conversation_id,
                    &attempt_id,
                    &message_id,
                    Some(&user_message_id),
                    Some(&provider_name),
                    Some(&model),
                    "error",
                    Some("gpu_gate_error"),
                    Some("gate_error"),
                    Some(&e.to_string()),
                    None,
                    true,
                );
                record_chat_attempt_trace(
                    &attempt_trace,
                    &trace_data_dir,
                    "gate_error",
                    Some(Duration::ZERO),
                    Some("GPU gate acquisition failed"),
                    Some(&e.to_string()),
                    |_| {},
                );
                app_state.finish_active_chat_attempt(&notebook_id, &conversation_id, &attempt_id);
                return;
            }
        };
        let _gpu_owner = app_state.gate_owner_guard("GPU gate", "chat", &message_id);

        let permit = match acquire_gate_with_epoch(
            &handle,
            &app_state,
            &notebook_id,
            &conversation_id,
            &message_id,
            epoch,
            &app_state.llm_gate,
            Duration::from_secs(120),
            "LLM gate",
        )
        .await
        {
            Ok(Some(permit)) => permit,
            Ok(None) => {
                execution_context.cancellation.cancel();
                app_state.clear_gate_owner("GPU gate", "chat");
                drop(gpu_permit);
                record_chat_attempt_trace(
                    &attempt_trace,
                    &trace_data_dir,
                    "cancelled",
                    Some(Duration::ZERO),
                    Some("LLM gate wait cancelled by notebook switch"),
                    Some("chat cancelled before acquiring LLM gate"),
                    |_| {},
                );
                persist_chat_attempt_status(
                    &app_state,
                    &notebook_id,
                    &conversation_id,
                    &attempt_id,
                    &message_id,
                    Some(&user_message_id),
                    Some(&provider_name),
                    Some(&model),
                    "cancelled",
                    Some("llm_gate_cancelled"),
                    Some("cancelled"),
                    Some("chat cancelled before acquiring LLM gate"),
                    None,
                    true,
                );
                app_state.finish_active_chat_attempt(&notebook_id, &conversation_id, &attempt_id);
                terminal.emit_cancelled("Chat cancelled before acquiring LLM gate");
                return;
            }
            Err(e) => {
                app_state.clear_gate_owner("GPU gate", "chat");
                drop(gpu_permit);
                terminal.emit_error(&e.to_string());
                persist_chat_attempt_status(
                    &app_state,
                    &notebook_id,
                    &conversation_id,
                    &attempt_id,
                    &message_id,
                    Some(&user_message_id),
                    Some(&provider_name),
                    Some(&model),
                    "error",
                    Some("llm_gate_error"),
                    Some("gate_error"),
                    Some(&e.to_string()),
                    None,
                    true,
                );
                record_chat_attempt_trace(
                    &attempt_trace,
                    &trace_data_dir,
                    "gate_error",
                    Some(Duration::ZERO),
                    Some("LLM gate acquisition failed"),
                    Some(&e.to_string()),
                    |_| {},
                );
                app_state.finish_active_chat_attempt(&notebook_id, &conversation_id, &attempt_id);
                return;
            }
        };
        let _llm_owner = app_state.gate_owner_guard("LLM gate", "chat", &message_id);

        let result = stream_chat_response(
            &handle,
            provider.as_ref(),
            &notebook_id,
            epoch,
            &conversation_id,
            &message_id,
            &query,
            &model,
            &history,
            custom_goal.as_deref(),
            &style,
            &response_length,
            &source_scope,
            &source_context,
            model_context_window,
            &attempt_trace,
            &trace_data_dir,
            execution_context.clone(),
        )
        .await;

        match &result {
            Ok(stream_result) => {
                let full_response = &stream_result.full_response;
                if !app_state.is_active_notebook_epoch(&notebook_id, epoch) {
                    // Notebook switched after stream completed — emit cancelled terminal
                    persist_chat_attempt_status(
                        &app_state,
                        &notebook_id,
                        &conversation_id,
                        &attempt_id,
                        &message_id,
                        Some(&user_message_id),
                        Some(&provider_name),
                        Some(&model),
                        "cancelled",
                        Some("late_cancelled_after_stream"),
                        Some("cancelled"),
                        Some(CHAT_CANCELLED_NOTEBOOK_SWITCH),
                        stream_result.generation_receipt.response_digest.as_deref(),
                        true,
                    );
                    terminal.emit_cancelled(CHAT_CANCELLED_NOTEBOOK_SWITCH);
                    app_state.clear_gate_owner("GPU gate", "chat");
                    app_state.clear_gate_owner("LLM gate", "chat");
                    drop(gpu_permit);
                    drop(permit);
                    app_state.finish_active_chat_attempt(
                        &notebook_id,
                        &conversation_id,
                        &attempt_id,
                    );
                    return;
                }

                // Extract citations from the response
                let (extracted, citation_filter_reasons) =
                    citations::extract_citations_from_context_with_reasons(
                        full_response,
                        &source_context,
                    );
                let citation_ref_count = citations::count_unique_citation_refs(full_response);
                let mut evidence = evidence_for_message.clone();
                evidence.citation_valid_count = extracted.len();
                evidence.citation_invalid_count =
                    citation_ref_count.saturating_sub(extracted.len());
                evidence.citation_filter_reasons = citation_filter_reasons;
                evidence.omitted_candidate_count =
                    source_context.len().saturating_sub(extracted.len());
                evidence.decoding_settings_receipt =
                    Some(stream_result.decoding_settings_receipt.clone());
                evidence.prompt_receipt = Some(stream_result.prompt_receipt.clone());
                evidence.generation_receipt = Some(stream_result.generation_receipt.clone());
                evidence.prompt_budget_receipt = stream_result.prompt_budget_receipt.clone();
                let citations_payload = AssistantMessageEvidence {
                    citations: extracted,
                    evidence,
                };
                let citations_json = serde_json::to_string(&citations_payload).ok();

                // Persist assistant message to DB — BEFORE chat:done.
                // If persistence fails, emit chat:error (not chat:done).
                let assistant_msg = Message {
                    id: message_id.clone(),
                    conversation_id: conversation_id.clone(),
                    role: "assistant".to_string(),
                    content: full_response.clone(),
                    citations: citations_json,
                    model_used: Some(model.clone()),
                    tokens_prompt: None,
                    tokens_response: None,
                    created_at: String::new(),
                };
                if let Err(e) = app_state
                    .with_notebook_db_write(&notebook_id, |db| db.insert_message(&assistant_msg))
                {
                    tracing::error!(message_id = %message_id, "Failed to persist assistant message: {}", e);
                    record_chat_attempt_trace(
                        &attempt_trace,
                        &trace_data_dir,
                        "assistant_persist_error",
                        Some(Duration::ZERO),
                        Some("Assistant message streaming completed but persistence failed"),
                        Some(&e.to_string()),
                        |_| {},
                    );
                    persist_chat_attempt_status(
                        &app_state,
                        &notebook_id,
                        &conversation_id,
                        &attempt_id,
                        &message_id,
                        Some(&user_message_id),
                        Some(&provider_name),
                        Some(&model),
                        "error",
                        Some("assistant_persist_error"),
                        Some("assistant_persist_error"),
                        Some(&e.to_string()),
                        stream_result.generation_receipt.response_digest.as_deref(),
                        true,
                    );
                    // DB insert failure → chat:error, NOT chat:done.
                    terminal.emit_error(&format!("Assistant message persistence failed: {e}"));
                    app_state.clear_gate_owner("GPU gate", "chat");
                    app_state.clear_gate_owner("LLM gate", "chat");
                    drop(gpu_permit);
                    drop(permit);
                    app_state.finish_active_chat_attempt(
                        &notebook_id,
                        &conversation_id,
                        &attempt_id,
                    );
                    return;
                } else {
                    let prompt_json = serde_json::to_string(&stream_result.prompt_receipt).ok();
                    let generation_json =
                        serde_json::to_string(&stream_result.generation_receipt).ok();
                    if let Some(prompt_json) = prompt_json.as_deref() {
                        if let Err(e) = app_state.with_notebook_db_write(&notebook_id, |db| {
                            db.insert_prompt_receipt(
                                &stream_result.prompt_receipt.receipt_id,
                                &notebook_id,
                                &conversation_id,
                                &message_id,
                                &stream_result.prompt_receipt.prompt_digest,
                                &stream_result.prompt_receipt.context_payload_digest,
                                prompt_json,
                            )
                        }) {
                            tracing::warn!(message_id = %message_id, error = %e, "Failed to persist PromptReceiptV1");
                        }
                    }
                    if let Some(generation_json) = generation_json.as_deref() {
                        if let Err(e) = app_state.with_notebook_db_write(&notebook_id, |db| {
                            db.insert_generation_receipt(
                                &stream_result.generation_receipt.receipt_id,
                                &notebook_id,
                                &conversation_id,
                                &message_id,
                                &stream_result.generation_receipt.provider,
                                &stream_result.generation_receipt.model,
                                &stream_result.generation_receipt.provider_request_digest,
                                stream_result.generation_receipt.response_digest.as_deref(),
                                &stream_result.generation_receipt.status,
                                generation_json,
                            )
                        }) {
                            tracing::warn!(message_id = %message_id, error = %e, "Failed to persist GenerationReceiptV1");
                        }
                    }
                    record_chat_attempt_trace(
                        &attempt_trace,
                        &trace_data_dir,
                        "assistant_persisted",
                        Some(Duration::ZERO),
                        Some("Assistant message persisted after provider completion"),
                        None,
                        |trace| {
                            trace.assistant_persisted = true;
                        },
                    );
                    persist_chat_attempt_status(
                        &app_state,
                        &notebook_id,
                        &conversation_id,
                        &attempt_id,
                        &message_id,
                        Some(&user_message_id),
                        Some(&provider_name),
                        Some(&model),
                        "succeeded",
                        Some("assistant_persisted"),
                        None,
                        None,
                        stream_result.generation_receipt.response_digest.as_deref(),
                        true,
                    );
                }
                // Assistant message persisted successfully — now emit evidence and chat:done.
                emit_chat_evidence(
                    &handle,
                    &notebook_id,
                    &conversation_id,
                    &message_id,
                    serde_json::to_value(&citations_payload.citations)
                        .unwrap_or_else(|_| serde_json::json!([])),
                    serde_json::to_value(&citations_payload.evidence)
                        .unwrap_or_else(|_| serde_json::json!({})),
                );
                terminal.emit_done();
            }
            Err(e) => {
                let cancellation_reason = e.to_string();
                if cancellation_reason == CHAT_CANCELLED_NOTEBOOK_SWITCH
                    || cancellation_reason == CHAT_CANCELLED_USER_REQUEST
                {
                    // Notebook-switch cancellation — emit chat:cancelled so frontend can clear state.
                    let terminal_reason = if cancellation_reason == CHAT_CANCELLED_NOTEBOOK_SWITCH {
                        CHAT_CANCELLED_NOTEBOOK_SWITCH
                    } else {
                        "Chat cancelled by user"
                    };
                    terminal.emit_cancelled(terminal_reason);
                    persist_chat_attempt_status(
                        &app_state,
                        &notebook_id,
                        &conversation_id,
                        &attempt_id,
                        &message_id,
                        Some(&user_message_id),
                        Some(&provider_name),
                        Some(&model),
                        "cancelled",
                        Some("stream_cancelled"),
                        Some("cancelled"),
                        Some(terminal_reason),
                        None,
                        true,
                    );
                    record_chat_attempt_trace(
                        &attempt_trace,
                        &trace_data_dir,
                        "cancelled",
                        Some(Duration::ZERO),
                        Some("Chat cancelled during streaming"),
                        Some(terminal_reason),
                        |_| {},
                    );
                } else {
                    let error_text = e.to_string();
                    tracing::error!(message_id = %message_id, "Chat streaming failed: {}", error_text);
                    emit_chat_status(
                        &handle,
                        &notebook_id,
                        &conversation_id,
                        &message_id,
                        "error",
                        "Chat request failed",
                        None,
                        Some(&model),
                        None,
                        None,
                        None,
                        Duration::ZERO,
                        None,
                        false,
                        Some(&error_text),
                    );
                    terminal.emit_error(&error_text);
                    persist_chat_attempt_status(
                        &app_state,
                        &notebook_id,
                        &conversation_id,
                        &attempt_id,
                        &message_id,
                        Some(&user_message_id),
                        Some(&provider_name),
                        Some(&model),
                        "error",
                        Some("stream_error"),
                        Some("provider_error"),
                        Some(&error_text),
                        None,
                        true,
                    );
                    record_chat_attempt_trace(
                        &attempt_trace,
                        &trace_data_dir,
                        "error",
                        Some(Duration::ZERO),
                        Some("Chat streaming failed"),
                        Some(&error_text),
                        |_| {},
                    );
                }
            }
        }

        // Release gates
        app_state.clear_gate_owner("GPU gate", "chat");
        app_state.clear_gate_owner("LLM gate", "chat");
        drop(gpu_permit);
        drop(permit);
        app_state.finish_active_chat_attempt(&notebook_id, &conversation_id, &attempt_id);
    }
}

#[allow(clippy::too_many_arguments)]
fn persist_chat_attempt_status(
    state: &AppState,
    notebook_id: &str,
    conversation_id: &str,
    attempt_id: &str,
    assistant_message_id: &str,
    user_message_id: Option<&str>,
    provider: Option<&str>,
    model: Option<&str>,
    status: &str,
    phase: Option<&str>,
    error_code: Option<&str>,
    error_message: Option<&str>,
    response_digest: Option<&str>,
    terminal: bool,
) {
    let attempt = ChatAttemptStatus {
        attempt_id: attempt_id.to_string(),
        notebook_id: notebook_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        user_message_id: user_message_id.map(str::to_string),
        provider: provider.map(str::to_string),
        model: model.map(str::to_string),
        status: status.to_string(),
        phase: phase.map(str::to_string),
        error_code: error_code.map(str::to_string),
        error_message: error_message.map(str::to_string),
        request_digest: None,
        response_digest: response_digest.map(str::to_string),
        partial_policy: Some("full_response_or_terminal_error".to_string()),
        terminal,
    };
    if let Err(err) =
        state.with_notebook_db_write(notebook_id, |db| db.upsert_chat_attempt_status(&attempt))
    {
        tracing::warn!(
            notebook_id,
            conversation_id,
            attempt_id,
            assistant_message_id,
            error = %err,
            "failed to persist chat_attempts status"
        );
    }
}

pub(crate) fn provider_decoding_capability(
    provider_type: providers::ProviderType,
) -> ProviderDecodingCapabilityV1 {
    match provider_type {
        providers::ProviderType::Ollama => ProviderDecodingCapabilityV1 {
            supports_temperature: true,
            supports_top_p: true,
            supports_top_k: true,
            supports_min_p: true,
            supports_repeat_penalty: true,
        },
        providers::ProviderType::LlamaCpp | providers::ProviderType::OpenAI => {
            ProviderDecodingCapabilityV1 {
                supports_temperature: true,
                supports_top_p: true,
                supports_top_k: false,
                supports_min_p: false,
                supports_repeat_penalty: false,
            }
        }
        providers::ProviderType::Anthropic => ProviderDecodingCapabilityV1 {
            // The current Anthropic adapter omits sampling controls to retain
            // compatibility with models that reject non-default parameters.
            supports_temperature: false,
            supports_top_p: false,
            supports_top_k: false,
            supports_min_p: false,
            supports_repeat_penalty: false,
        },
    }
}

pub(crate) fn parse_optional_f32(value: Option<String>) -> Option<f32> {
    value
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
}

pub(crate) fn parse_optional_i64(value: Option<String>) -> Option<i64> {
    value.and_then(|value| value.trim().parse::<i64>().ok())
}

pub(crate) fn effective_decoding_settings(
    state: &AppState,
    provider_type: providers::ProviderType,
    model: &str,
    max_tokens: u32,
) -> Result<DecodingSettingsReceiptV1, GlossError> {
    let capability = provider_decoding_capability(provider_type);
    let (
        requested_temperature,
        requested_top_p,
        requested_top_k,
        requested_min_p,
        requested_repeat_penalty,
    ) = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        (
            app_db.get_setting("generation_temperature")?,
            app_db.get_setting("generation_top_p")?,
            app_db.get_setting("generation_top_k")?,
            app_db.get_setting("generation_min_p")?,
            app_db.get_setting("generation_repeat_penalty")?,
        )
    };
    let requested = serde_json::json!({
        "temperature": requested_temperature,
        "top_p": requested_top_p,
        "top_k": requested_top_k,
        "min_p": requested_min_p,
        "repeat_penalty": requested_repeat_penalty,
    });
    let mut unsupported_fields = Vec::new();
    let temperature = if capability.supports_temperature {
        parse_optional_f32(requested["temperature"].as_str().map(str::to_string))
            .unwrap_or(DEFAULT_CHAT_TEMPERATURE)
            .clamp(0.0, 2.0)
    } else {
        if requested["temperature"]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty())
        {
            unsupported_fields.push("temperature".to_string());
        }
        // Anthropic omits this field: record its documented Messages API
        // default, not the saved value that never reaches the provider.
        // https://platform.claude.com/docs/en/api/messages/create
        1.0
    };
    let top_p = if capability.supports_top_p {
        parse_optional_f32(requested["top_p"].as_str().map(str::to_string))
    } else {
        if requested["top_p"]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty())
        {
            unsupported_fields.push("top_p".to_string());
        }
        None
    };
    let top_k = if capability.supports_top_k {
        parse_optional_i64(requested["top_k"].as_str().map(str::to_string))
    } else {
        if requested["top_k"]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty())
        {
            unsupported_fields.push("top_k".to_string());
        }
        None
    };
    let min_p = if capability.supports_min_p {
        parse_optional_f32(requested["min_p"].as_str().map(str::to_string))
    } else {
        if requested["min_p"]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty())
        {
            unsupported_fields.push("min_p".to_string());
        }
        None
    };
    let repeat_penalty = if capability.supports_repeat_penalty {
        parse_optional_f32(requested["repeat_penalty"].as_str().map(str::to_string))
    } else {
        if requested["repeat_penalty"]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty())
        {
            unsupported_fields.push("repeat_penalty".to_string());
        }
        None
    };

    Ok(DecodingSettingsReceiptV1 {
        schema: "DecodingSettingsReceiptV1".to_string(),
        receipt_id: uuid::Uuid::new_v4().to_string(),
        provider: provider_type.as_str().to_string(),
        model: model.to_string(),
        requested,
        effective: EffectiveDecodingSettingsV1 {
            temperature,
            top_p,
            top_k,
            min_p,
            repeat_penalty,
            max_tokens,
        },
        unsupported_fields,
        provider_capability: capability,
        recorded_at: chrono::Utc::now().to_rfc3339(),
    })
}

#[derive(Debug, Clone, Serialize)]
struct ProjectionReadiness {
    ready: bool,
    reason_code: Option<RetrievalReasonCode>,
    user_action: Option<String>,
    scoped_sources: usize,
    scoped_chunks: usize,
    healthy_links: usize,
    missing_links: usize,
    skipped_no_chunks: usize,
}

#[cfg(feature = "semantic-memory-backend")]
fn semantic_memory_search_timeout_from_setting(value: Option<String>) -> Duration {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|millis| *millis > 0)
        .map(Duration::from_millis)
        .unwrap_or(SEMANTIC_MEMORY_SEARCH_TIMEOUT)
}

pub(crate) fn setting_is_enabled(value: Option<String>) -> bool {
    value
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            normalized == "1" || normalized == "true" || normalized == "yes"
        })
        .unwrap_or(false)
}

fn bounded_semantic_fallback_reason(reason: &str) -> &'static str {
    let normalized = reason.trim().to_ascii_lowercase();
    if normalized.contains("timeout") {
        "search-timeout"
    } else if normalized.contains("not compiled") || normalized.contains("feature is not enabled") {
        "semantic-memory-not-compiled"
    } else if normalized.contains("flag") || normalized.contains("gate") {
        "semantic-memory-flag-off"
    } else if normalized.contains("link") || normalized.contains("backpointer") {
        "links-missing"
    } else if normalized.contains("artifact")
        || normalized.contains("turbo")
        || normalized.contains("derived_vector")
    {
        "turbo-artifacts-stale"
    } else if normalized.contains("candidate") || normalized.contains("empty") {
        "no-candidates"
    } else {
        "projection-failed"
    }
}

fn retrieval_reason_code_from_text(value: &str) -> Option<RetrievalReasonCode> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "search-timeout"
        | "semantic_memory_timeout"
        | "semantic-memory-preview-timeout-fallback" => {
            Some(RetrievalReasonCode::SemanticMemoryTimeout)
        }
        "semantic-memory-not-compiled" | "semantic_memory_build_feature_missing" => {
            Some(RetrievalReasonCode::SemanticMemoryBuildFeatureMissing)
        }
        "semantic-memory-flag-off"
        | "semantic_memory_feature_disabled"
        | "semantic-memory-feature-disabled-fallback" => {
            Some(RetrievalReasonCode::SemanticMemoryFeatureDisabled)
        }
        "links-missing" | "projection-required" | "semantic_memory_links_missing" => {
            Some(RetrievalReasonCode::SemanticMemoryLinksMissing)
        }
        "projection-failed" | "semantic_memory_links_degraded" => {
            Some(RetrievalReasonCode::SemanticMemoryLinksDegraded)
        }
        "turbo-artifacts-stale" | "embedding_index_metadata_stale" => {
            Some(RetrievalReasonCode::EmbeddingIndexMetadataStale)
        }
        "no-candidates" | "no_retrieval_context" | "semantic-memory-empty-context" => {
            Some(RetrievalReasonCode::NoRetrievalContext)
        }
        "source-order-fallback" | "source_order_fallback" => {
            Some(RetrievalReasonCode::SourceOrderFallback)
        }
        "raw-content-fallback" | "raw_content_fallback" => {
            Some(RetrievalReasonCode::RawContentFallback)
        }
        "index_missing" => Some(RetrievalReasonCode::IndexMissing),
        "bm25_no_matches" => Some(RetrievalReasonCode::Bm25NoMatches),
        "bm25_query_sanitized_empty" => Some(RetrievalReasonCode::Bm25QuerySanitizedEmpty),
        _ if (normalized.contains("metadata") || normalized.contains("embedding index"))
            && normalized.contains("stale") =>
        {
            Some(RetrievalReasonCode::EmbeddingIndexMetadataStale)
        }
        _ if normalized.contains("timeout") => Some(RetrievalReasonCode::SemanticMemoryTimeout),
        _ if normalized.contains("no mapped candidates") => {
            Some(RetrievalReasonCode::NoRetrievalContext)
        }
        _ if normalized.contains("projection") && normalized.contains("required") => {
            Some(RetrievalReasonCode::SemanticMemoryLinksMissing)
        }
        _ if normalized.contains("projection") || normalized.contains("link") => {
            Some(RetrievalReasonCode::SemanticMemoryLinksDegraded)
        }
        _ => None,
    }
}

fn load_cached_suggested_questions(
    notebook_id: &str,
    state: &AppState,
) -> Result<Vec<String>, GlossError> {
    let cached = state.with_notebook_db(notebook_id, |db| db.get_config("suggested_questions"))?;
    if let Some(json) = cached {
        let parsed: Vec<String> = serde_json::from_str(&json).unwrap_or_default();
        if !parsed.is_empty() {
            return Ok(parsed);
        }
    }
    Ok(Vec::new())
}

fn estimate_tokens(text: &str) -> u32 {
    let chars = text.chars().count() as u32;
    (chars / 4).max(1)
}

fn compute_dynamic_num_ctx(
    system_prompt: &str,
    messages: &[ChatMessage],
    model_context_window: Option<i32>,
    max_tokens: u32,
) -> ContextBudgetResult {
    let prompt_tokens = estimate_tokens(system_prompt)
        + messages
            .iter()
            .map(|message| estimate_tokens(&message.content))
            .sum::<u32>();
    let needed = prompt_tokens
        .saturating_add(max_tokens)
        .saturating_add(1_024);
    let model_limit = model_context_window
        .and_then(|window| u32::try_from(window).ok())
        .filter(|window| *window > 0)
        .unwrap_or(DEFAULT_CONTEXT_WINDOW_TOKENS)
        .min(MAX_CONTEXT_WINDOW_TOKENS);

    let num_ctx = needed.clamp(DEFAULT_CONTEXT_WINDOW_TOKENS.min(model_limit), model_limit);
    ContextBudgetResult {
        num_ctx,
        needed,
        prompt_tokens,
        context_budgeted: needed > model_limit,
    }
}

struct ContextBudgetResult {
    num_ctx: u32,
    needed: u32,
    prompt_tokens: u32,
    context_budgeted: bool,
}

#[tauri::command]
pub async fn list_conversations(
    notebook_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<Conversation>, GlossError> {
    state.with_notebook_db(&notebook_id, |db| db.list_conversations())
}

#[tauri::command]
pub async fn create_conversation(
    notebook_id: String,
    state: State<'_, AppState>,
) -> Result<String, GlossError> {
    let id = uuid::Uuid::new_v4().to_string();
    state.with_notebook_db_write(&notebook_id, |db| db.create_conversation(&id))?;
    Ok(id)
}

#[tauri::command]
pub async fn delete_conversation(
    notebook_id: String,
    conversation_id: String,
    state: State<'_, AppState>,
) -> Result<(), GlossError> {
    state.with_notebook_db_write(&notebook_id, |db| db.delete_conversation(&conversation_id))
}

#[tauri::command]
pub async fn stop_chat(
    notebook_id: String,
    state: State<'_, AppState>,
) -> Result<StopChatResponseV1, GlossError> {
    let cancelled = state.cancel_active_chats_for_notebook(&notebook_id);
    let attempts = cancelled
        .iter()
        .map(|attempt| ChatCancellationRequestV1 {
            attempt_id: attempt.attempt_id.clone(),
            conversation_id: attempt.conversation_id.clone(),
            message_id: attempt.message_id.clone(),
        })
        .collect::<Vec<_>>();
    if !cancelled.is_empty() {
        tracing::info!(
            notebook_id = %notebook_id,
            attempts = cancelled.len(),
            conversations = ?cancelled.iter().map(|attempt| attempt.conversation_id.as_str()).collect::<Vec<_>>(),
            message_ids = ?cancelled.iter().map(|attempt| attempt.message_id.as_str()).collect::<Vec<_>>(),
            "Cancellation requested for active chat attempt(s)"
        );
        state.bump_user_activity();
    }
    Ok(StopChatResponseV1 {
        cancellation_requested: !attempts.is_empty(),
        attempts,
    })
}

#[tauri::command]
pub async fn load_messages(
    notebook_id: String,
    conversation_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<Message>, GlossError> {
    state.with_notebook_db(&notebook_id, |db| db.load_messages(&conversation_id))
}

#[tauri::command]
pub async fn get_chat_events_since(
    notebook_id: String,
    conversation_id: String,
    after_seq: Option<u64>,
    state: State<'_, AppState>,
) -> Result<Vec<ChatStreamEventV1>, GlossError> {
    Ok(state.chat_events_since(&notebook_id, &conversation_id, after_seq))
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn send_message(
    notebook_id: String,
    conversation_id: String,
    query: String,
    source_scope: SourceScope,
    model: String,
    message_id: Option<String>,
    style: Option<String>,
    custom_goal: Option<String>,
    response_length: Option<String>,
    history_before_user_message_id: Option<String>,
    user_message_id: Option<String>,
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
    app_handle: tauri::AppHandle,
) -> Result<String, GlossError> {
    let message_id = message_id
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let user_message_id = resolve_user_message_id(user_message_id, &message_id)?;
    // Reject an invalid edit target before accepting an attempt or preempting
    // background work. Retain this validated request-only prefix; ordinary
    // sends still load their full history at the existing context boundary.
    let rerun_history = match history_before_user_message_id.as_deref() {
        Some(target) => Some(history_before_rerun(
            state.with_notebook_db(&notebook_id, |db| db.load_messages(&conversation_id))?,
            &conversation_id,
            Some(target),
        )?),
        None => None,
    };
    let (trace_memory_backend, trace_memory_fallback) = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        (
            app_db.get_setting("memory_backend")?,
            app_db
                .get_setting("memory_backend_fallback")?
                .map(|value| setting_is_enabled(Some(value))),
        )
    };
    let trace_source_scope_mode = match &source_scope {
        SourceScope::All => "all",
        SourceScope::Explicit(_) => "explicit",
        SourceScope::None => "none",
    }
    .to_string();
    let attempt_trace = Arc::new(Mutex::new(new_chat_attempt_trace(
        &notebook_id,
        &conversation_id,
        &message_id,
        &model,
        trace_memory_backend,
        trace_memory_fallback,
        Some(trace_source_scope_mode),
    )));
    let attempt_id = chat_attempt_trace_snapshot(&attempt_trace).attempt_id;
    let trace_data_dir = state.data_dir.clone();
    record_chat_attempt_trace(
        &attempt_trace,
        &trace_data_dir,
        "queued",
        Some(Duration::ZERO),
        Some("Chat request accepted by Tauri command"),
        None,
        |_| {},
    );
    // Register before *any* normal user-turn side effect. The lease clears the
    // registration on every pre-stream error path; ownership transfers to the
    // spawned stream task only after context construction succeeds.
    let active_chat_attempt_lease = match state.lease_active_chat_attempt(
        &notebook_id,
        &conversation_id,
        &attempt_id,
        &user_message_id,
    ) {
        Ok(lease) => lease,
        Err(existing) => {
            let error = format!(
                "Conversation already has an active chat attempt: {}",
                existing.attempt_id
            );
            emit_chat_status(
                &app_handle,
                &notebook_id,
                &conversation_id,
                &message_id,
                "single_flight_rejected",
                "Conversation already has an active chat",
                None,
                Some(&model),
                None,
                None,
                None,
                Duration::ZERO,
                None,
                false,
                Some(&error),
            );
            emit_chat_error(
                &app_handle,
                &notebook_id,
                &conversation_id,
                &message_id,
                &error,
            );
            record_chat_attempt_trace(
                &attempt_trace,
                &trace_data_dir,
                "single_flight_rejected",
                Some(Duration::ZERO),
                Some("Concurrent chat generation rejected before user-turn persistence"),
                Some(&error),
                |_| {},
            );
            persist_chat_attempt_status(
                &state,
                &notebook_id,
                &conversation_id,
                &attempt_id,
                &message_id,
                None,
                None,
                Some(&model),
                "error",
                Some("single_flight_rejected"),
                Some("single_flight_rejected"),
                Some(&error),
                None,
                true,
            );
            return Err(GlossError::Other(error));
        }
    };
    // ChatAttemptStatus is the durable terminal ledger; the replay ring buffer
    // is transport-only. Targeted cancellation is handled by the active
    // attempt's CancellationToken once provider generation starts.
    persist_chat_attempt_status(
        &state,
        &notebook_id,
        &conversation_id,
        &attempt_id,
        &message_id,
        None,
        None,
        Some(&model),
        "queued",
        Some("queued"),
        None,
        None,
        None,
        false,
    );

    if state.get_active_notebook_id().as_deref() != Some(notebook_id.as_str()) {
        if let Err(err) = {
            let app_db = state
                .app_db
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            app_db.get_notebook(&notebook_id).map(|_| ())
        } {
            let error = format!("Requested notebook is not available for chat: {err}");
            emit_chat_status(
                &app_handle,
                &notebook_id,
                &conversation_id,
                &message_id,
                "active_notebook_error",
                "Requested notebook is not active",
                None,
                Some(&model),
                None,
                None,
                None,
                Duration::ZERO,
                None,
                false,
                Some(&error),
            );
            emit_chat_error(
                &app_handle,
                &notebook_id,
                &conversation_id,
                &message_id,
                &error,
            );
            record_chat_attempt_trace(
                &attempt_trace,
                &trace_data_dir,
                "active_notebook_error",
                Some(Duration::ZERO),
                Some("Requested notebook is not active or available"),
                Some(&error),
                |_| {},
            );
            persist_chat_attempt_status(
                &state,
                &notebook_id,
                &conversation_id,
                &attempt_id,
                &message_id,
                None,
                None,
                Some(&model),
                "error",
                Some("active_notebook_error"),
                Some("active_notebook_error"),
                Some(&error),
                None,
                true,
            );
            return Err(GlossError::Other(error));
        }
        state.set_active_notebook(Some(notebook_id.clone()), None);
        emit_chat_status(
            &app_handle,
            &notebook_id,
            &conversation_id,
            &message_id,
            "active_notebook_set",
            "Activated requested notebook for chat",
            None,
            Some(&model),
            None,
            None,
            None,
            Duration::ZERO,
            None,
            false,
            None,
        );
    }
    let request_epoch = state.get_active_epoch();

    // Chat preemption begins at user message arrival, not after RAG assembly.
    state.bump_chat_grace();
    state.bump_user_activity();

    let cancelled = jobs::cancel_jobs_matching(&queue, |job, status| {
        status == "processing" && job.notebook_id() == notebook_id
    });
    if cancelled > 0 {
        tracing::info!(
            cancelled,
            "Cancelled in-flight background jobs for chat preemption"
        );
    }

    // Load history BEFORE inserting user message to avoid duplicate
    // style and custom_goal come from the frontend as command parameters (per-conversation),
    // not from notebook_config global settings.
    let effective_style = style
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "default".to_string());
    let effective_custom_goal = custom_goal.filter(|g| !g.trim().is_empty());
    let effective_response_length = response_length
        .filter(|r| !r.trim().is_empty())
        .unwrap_or_else(|| "default".to_string());
    let history = match rerun_history {
        Some(history) => history,
        None => state.with_notebook_db(&notebook_id, |db| db.load_messages(&conversation_id))?,
    };

    // Store user message
    let user_msg = Message {
        id: user_message_id,
        conversation_id: conversation_id.clone(),
        role: "user".to_string(),
        content: query.clone(),
        citations: None,
        model_used: None,
        tokens_prompt: None,
        tokens_response: None,
        created_at: String::new(),
    };
    state.with_notebook_db_write(&notebook_id, |db| db.insert_message(&user_msg))?;
    persist_chat_attempt_status(
        &state,
        &notebook_id,
        &conversation_id,
        &attempt_id,
        &message_id,
        Some(&user_msg.id),
        None,
        Some(&model),
        "running",
        Some("user_message_persisted"),
        None,
        None,
        None,
        false,
    );
    record_chat_attempt_trace(
        &attempt_trace,
        &trace_data_dir,
        "user_message_persisted",
        Some(Duration::ZERO),
        Some("User message persisted; resolving provider config"),
        None,
        |_| {},
    );

    // Get provider config (short lock, no await)
    let provider_lookup: Result<(providers::ProviderConfig, Option<i32>), GlossError> = {
        let registry = state
            .model_registry
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let provider_config =
            registry.get_provider_config_for_model(&model, &app_db, &state.secret_store)?;
        let selected_provider_id = app_db.get_setting("default_provider")?;
        let model_context_window = app_db
            .get_all_models()?
            .into_iter()
            .find(|record| {
                record.id == model
                    && selected_provider_id
                        .as_deref()
                        .map(|provider_id| provider_id == record.provider_id)
                        .unwrap_or(true)
            })
            .and_then(|record| record.context_window);
        Ok((provider_config, model_context_window))
    };
    let (provider_config, model_context_window) = match provider_lookup {
        Ok(value) => value,
        Err(err) => {
            let error = err.to_string();
            emit_chat_status(
                &app_handle,
                &notebook_id,
                &conversation_id,
                &message_id,
                "provider_config_error",
                "Provider configuration failed",
                None,
                Some(&model),
                None,
                None,
                None,
                Duration::ZERO,
                None,
                false,
                Some(&error),
            );
            emit_chat_error(
                &app_handle,
                &notebook_id,
                &conversation_id,
                &message_id,
                &error,
            );
            record_chat_attempt_trace(
                &attempt_trace,
                &trace_data_dir,
                "provider_config_error",
                Some(Duration::ZERO),
                Some("Provider configuration failed before retrieval"),
                Some(&error),
                |_| {},
            );
            persist_chat_attempt_status(
                &state,
                &notebook_id,
                &conversation_id,
                &attempt_id,
                &message_id,
                Some(&user_msg.id),
                None,
                Some(&model),
                "error",
                Some("provider_config_error"),
                Some("provider_config_error"),
                Some(&error),
                None,
                true,
            );
            return Err(err);
        }
    };
    record_chat_attempt_trace(
        &attempt_trace,
        &trace_data_dir,
        "provider_config_resolved",
        Some(Duration::ZERO),
        Some("Provider config resolved from provider table"),
        None,
        |trace| {
            trace.provider = provider_config.provider_type.as_str().to_string();
            trace.provider_base_url = Some(provider_config.base_url.clone());
        },
    );
    persist_chat_attempt_status(
        &state,
        &notebook_id,
        &conversation_id,
        &attempt_id,
        &message_id,
        Some(&user_msg.id),
        Some(provider_config.provider_type.as_str()),
        Some(&model),
        "running",
        Some("provider_config_resolved"),
        None,
        None,
        None,
        false,
    );

    // --- RAG context assembly ---
    // 1. Load notebook sources upfront for scope resolution and manifest assembly.
    let all_sources: Vec<Source> = state.with_notebook_db(&notebook_id, |db| db.list_sources())?;
    let resolved_scope = source_scope.resolve(&all_sources);
    let requested_source_ids = match &source_scope {
        SourceScope::All | SourceScope::None => Vec::new(),
        SourceScope::Explicit(ids) => ids.clone(),
    };
    let effective_source_ids = resolved_scope.source_ids().to_vec();
    let effective_source_set = effective_source_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let invalid_source_ids = requested_source_ids
        .iter()
        .filter(|id| !id.trim().is_empty())
        .filter(|id| !effective_source_set.contains(id.as_str()))
        .cloned()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let excluded_source_ids = all_sources
        .iter()
        .filter(|source| !resolved_scope.allows(&source.id))
        .map(|source| source.id.clone())
        .collect::<Vec<_>>();
    let excluded_source_count = excluded_source_ids.len();
    let source_scope_mode = match resolved_scope.kind() {
        crate::retrieval::source_scope::ResolvedSourceScopeKind::All => "all",
        crate::retrieval::source_scope::ResolvedSourceScopeKind::Explicit => "explicit",
        crate::retrieval::source_scope::ResolvedSourceScopeKind::None => "none",
    }
    .to_string();

    if matches!(source_scope, SourceScope::Explicit(_)) && resolved_scope.is_none() {
        tracing::warn!(
            notebook_sources = all_sources.len(),
            "Scoped chat request resolved to no valid sources"
        );
    }

    let native_dense_possible =
        crate::state::NATIVE_SEMANTIC_INDEXING_ENABLED && !resolved_scope.is_none();

    // 3. Only initialize semantic search infrastructure when the selected
    // sources can use native dense retrieval. BM25/FTS5 still runs even when
    // dense retrieval is unavailable or only partially covered.
    if native_dense_possible {
        if let Err(e) = state.ensure_embedder(Some(&app_handle)) {
            tracing::warn!(
                "Embedder init failed (will fall back to raw context): {}",
                e
            );
        }
        if let Err(e) = state.ensure_hnsw_index(&notebook_id) {
            tracing::warn!(
                "HNSW index init failed (will fall back to raw context): {}",
                e
            );
        }
    } else {
        tracing::info!(
            notebook_id = %notebook_id,
            selected_sources = resolved_scope.source_count(),
            "Skipping native dense search warmup because native indexing is disabled or scope is empty"
        );
    }

    if active_chat_attempt_lease
        .cancellation()
        .map(|c| c.is_cancelled())
        .unwrap_or(true)
    {
        let error = "Chat cancelled during retrieval setup";
        emit_chat_status(
            &app_handle,
            &notebook_id,
            &conversation_id,
            &message_id,
            "cancelled",
            error,
            None,
            Some(&model),
            None,
            None,
            None,
            Duration::ZERO,
            None,
            false,
            Some(error),
        );
        emit_chat_error(
            &app_handle,
            &notebook_id,
            &conversation_id,
            &message_id,
            error,
        );
        record_chat_attempt_trace(
            &attempt_trace,
            &trace_data_dir,
            "cancelled",
            Some(Duration::ZERO),
            Some("Stop requested during retrieval setup"),
            Some(error),
            |_| {},
        );
        persist_chat_attempt_status(
            &state,
            &notebook_id,
            &conversation_id,
            &attempt_id,
            &message_id,
            Some(&user_msg.id),
            None,
            Some(&model),
            "cancelled",
            Some("cancelled_during_retrieval_setup"),
            Some("cancelled"),
            Some(error),
            None,
            true,
        );
        return Err(GlossError::Other(error.to_string()));
    }

    let source_count = resolved_scope.source_count();
    let top_k = hybrid_search::compute_top_k(source_count);

    tracing::info!(
        scope_kind = ?resolved_scope.kind(),
        scoped_sources = resolved_scope.source_count(),
        source_count,
        top_k,
        "Starting RAG context assembly"
    );

    #[cfg(feature = "semantic-memory-backend")]
    let (
        memory_backend,
        semantic_fallback_allowed,
        semantic_memory_runtime_config,
        semantic_memory_search_timeout,
    ) = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        (
            app_db
                .get_setting("memory_backend")?
                .unwrap_or_else(|| MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string()),
            setting_is_enabled(app_db.get_setting("memory_backend_fallback")?),
            semantic_memory_adapter::runtime_config_from_settings(
                app_db.get_setting("semantic_memory_embedding_provider")?,
                app_db.get_setting("semantic_memory_embedding_url")?,
                app_db.get_setting("semantic_memory_embedding_model")?,
                app_db.get_setting("semantic_memory_embedding_timeout_secs")?,
                crate::providers::lan_local_providers_allowed(&app_db),
                setting_is_enabled(app_db.get_setting(features::FASTEMBED_DOWNLOAD_CONSENT)?),
                features::turbo_quant_active(&app_db)?,
                setting_is_enabled(
                    app_db.get_setting(
                        features::SEMANTIC_MEMORY_TURBO_QUANT_REQUIRE_FRESH_ARTIFACTS,
                    )?,
                ),
                setting_is_enabled(
                    app_db
                        .get_setting(features::SEMANTIC_MEMORY_PROVEKV_POOL_CANDIDATES_ENABLED)?,
                ),
            ),
            semantic_memory_search_timeout_from_setting(
                app_db.get_setting("semantic_memory_search_timeout_ms")?,
            ),
        )
    };
    #[cfg(not(feature = "semantic-memory-backend"))]
    let (memory_backend, semantic_fallback_allowed) = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        (
            app_db
                .get_setting("memory_backend")?
                .unwrap_or_else(|| "gloss-local".to_string()),
            setting_is_enabled(app_db.get_setting("memory_backend_fallback")?),
        )
    };

    let retrieval_backend_requested = memory_backend.clone();
    let mut retrieval_backend_used = MEMORY_BACKEND_GLOSS_LOCAL.to_string();
    let mut retrieval_fallback_reason: Option<String> = None;
    let mut retrieval_degradation_markers: Vec<String> = Vec::new();
    let mut retrieval_mode = MEMORY_BACKEND_GLOSS_LOCAL.to_string();
    let mut force_gloss_local_retrieval = false;
    let mut retrieval_outcome_for_evidence: Option<RetrievalOutcome> = None;
    let retrieval_receipt_id = uuid::Uuid::new_v4().to_string();
    #[cfg(feature = "semantic-memory-backend")]
    let mut semantic_memory_receipt: Option<serde_json::Value> = None;
    #[cfg(not(feature = "semantic-memory-backend"))]
    let semantic_memory_receipt: Option<serde_json::Value> = None;
    if !invalid_source_ids.is_empty() {
        retrieval_degradation_markers.push("source-scope-partial-invalid".to_string());
    }
    let semantic_preview_gate_open = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        features::semantic_memory_preview_active(&app_db)?
    };
    let memory_backend = if memory_backend == MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW
        && !semantic_preview_gate_open
    {
        if semantic_fallback_allowed {
            retrieval_fallback_reason = Some("semantic-memory-flag-off".to_string());
            retrieval_degradation_markers.push("semantic_memory_feature_disabled".to_string());
            force_gloss_local_retrieval = true;
            MEMORY_BACKEND_GLOSS_LOCAL.to_string()
        } else {
            let error =
                "semantic-memory-flag-off: semantic-memory preview is disabled by the runtime feature gate"
                    .to_string();
            emit_chat_status(
                &app_handle,
                &notebook_id,
                &conversation_id,
                &message_id,
                "semantic_memory_search_error",
                "semantic-memory preview is disabled",
                None,
                Some(&model),
                None,
                Some("semantic-memory"),
                None,
                Duration::ZERO,
                None,
                false,
                Some(&error),
            );
            emit_chat_error(
                &app_handle,
                &notebook_id,
                &conversation_id,
                &message_id,
                &error,
            );
            record_chat_attempt_trace(
                &attempt_trace,
                &trace_data_dir,
                "semantic_memory_search_error",
                Some(Duration::ZERO),
                Some("strict semantic-memory mode failed before local fallback"),
                Some(&error),
                |_| {},
            );
            return Err(GlossError::Config(error));
        }
    } else {
        memory_backend
    };

    let semantic_preview_context: Option<Vec<ContextPassage>> = if memory_backend
        == MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW
        && !resolved_scope.is_none()
    {
        #[cfg(feature = "semantic-memory-backend")]
        {
            let db_path = state.notebook_db_path(&notebook_id)?;
            let links = {
                let nb_db = NotebookDb::connect(&db_path)?;
                semantic_memory_adapter::load_links(&nb_db)?
            };
            let projection_summary = {
                let nb_db = NotebookDb::connect(&db_path)?;
                nb_db.semantic_memory_projection_summary(&notebook_id, &resolved_scope)?
            };
            let semantic_embedding_metadata = {
                let nb_db = NotebookDb::connect(&db_path)?;
                nb_db.embedding_index_metadata(crate::db::notebook_db::SEMANTIC_MEMORY_INDEX_ID)?
            };
            let scoped_links = links
                .iter()
                .filter(|link| resolved_scope.allows(&link.source_id))
                .collect::<Vec<_>>();
            let healthy_scoped_links = scoped_links
                .iter()
                .filter(|link| {
                    link.sync_status == "synced"
                        && link
                            .sm_document_id
                            .as_deref()
                            .is_some_and(|value| !value.trim().is_empty())
                        && link
                            .sm_chunk_id
                            .as_deref()
                            .is_some_and(|value| !value.trim().is_empty())
                        && !link.content_digest.trim().is_empty()
                })
                .count();
            let scoped_link_count = scoped_links.len();
            let scoped_degraded_link_count = scoped_link_count.saturating_sub(healthy_scoped_links);
            let projection_readiness = ProjectionReadiness {
                ready: projection_summary.total_chunks > 0
                    && !projection_summary.projection_required
                    && projection_summary.missing_links == 0,
                reason_code: if projection_summary.missing_links > 0 || scoped_link_count == 0 {
                    Some(RetrievalReasonCode::SemanticMemoryLinksMissing)
                } else if scoped_degraded_link_count > 0 || projection_summary.degraded_links > 0 {
                    Some(RetrievalReasonCode::SemanticMemoryLinksDegraded)
                } else {
                    None
                },
                user_action: if projection_summary.projection_required {
                    Some("Run semantic-memory backfill or enable fallback.".to_string())
                } else {
                    None
                },
                scoped_sources: projection_summary.total_sources,
                scoped_chunks: projection_summary.total_chunks,
                healthy_links: projection_summary.healthy_links.max(healthy_scoped_links),
                missing_links: projection_summary.missing_links,
                skipped_no_chunks: projection_summary.skipped_no_chunks,
            };
            if !projection_readiness.ready && projection_readiness.missing_links > 0 {
                retrieval_degradation_markers.push(
                    RetrievalReasonCode::SemanticMemoryLinksMissing
                        .as_str()
                        .to_string(),
                );
                retrieval_fallback_reason.get_or_insert_with(|| "projection-required".to_string());
            } else if !projection_readiness.ready
                && (healthy_scoped_links < scoped_link_count
                    || projection_summary.degraded_links > 0
                    || projection_summary.failed_sources > 0)
            {
                retrieval_degradation_markers.push(
                    RetrievalReasonCode::SemanticMemoryLinksDegraded
                        .as_str()
                        .to_string(),
                );
                retrieval_fallback_reason.get_or_insert_with(|| "projection-failed".to_string());
            }
            if retrieval_fallback_reason
                .as_deref()
                .is_some_and(|reason| matches!(reason, "projection-required" | "projection-failed"))
            {
                if semantic_fallback_allowed {
                    force_gloss_local_retrieval = true;
                    emit_chat_status(
                        &app_handle,
                        &notebook_id,
                        &conversation_id,
                        &message_id,
                        "semantic_memory_search_fallback",
                        "Falling back to Gloss local retrieval",
                        None,
                        Some(&model),
                        None,
                        Some("gloss-local"),
                        retrieval_fallback_reason.as_deref(),
                        Duration::ZERO,
                        None,
                        true,
                        None,
                    );
                    record_chat_attempt_trace(
                        &attempt_trace,
                        &trace_data_dir,
                        "semantic_memory_search_fallback",
                        Some(Duration::ZERO),
                        retrieval_fallback_reason.as_deref(),
                        None,
                        |_| {},
                    );
                    None
                } else {
                    let error = format!(
                        "semantic-memory strict mode blocked: projection required for selected scope ({}; scoped_sources={}, scoped_chunks={}, healthy_links={}, missing_links={}, skipped_no_chunks={}). {}",
                        retrieval_fallback_reason
                            .as_deref()
                            .unwrap_or("projection-failed"),
                        projection_readiness.scoped_sources,
                        projection_readiness.scoped_chunks,
                        projection_readiness.healthy_links,
                        projection_readiness.missing_links,
                        projection_readiness.skipped_no_chunks,
                        projection_readiness
                            .user_action
                            .as_deref()
                            .unwrap_or("Run semantic-memory backfill or enable fallback.")
                    );
                    emit_chat_status(
                        &app_handle,
                        &notebook_id,
                        &conversation_id,
                        &message_id,
                        "semantic_memory_search_error",
                        "semantic-memory projection is not ready",
                        None,
                        Some(&model),
                        None,
                        Some("semantic-memory"),
                        None,
                        Duration::ZERO,
                        None,
                        false,
                        Some(&error),
                    );
                    emit_chat_error(
                        &app_handle,
                        &notebook_id,
                        &conversation_id,
                        &message_id,
                        &error,
                    );
                    record_chat_attempt_trace(
                        &attempt_trace,
                        &trace_data_dir,
                        "semantic_memory_search_error",
                        Some(Duration::ZERO),
                        Some("strict semantic-memory mode failed because projection links are not ready"),
                        Some(&error),
                        |_| {},
                    );
                    return Err(GlossError::Search(error));
                }
            } else {
                let preview_request = MemorySearchRequest {
                    notebook_id: notebook_id.clone(),
                    source_scope: source_scope.clone(),
                    query: query.clone(),
                    limit: top_k,
                    trace_id: Some(retrieval_receipt_id.clone()),
                    // allow_fallback: true — semantic_fallback_allowed controls whether
                    // the memory backend may degrade to gloss-local when preview fails.
                    allow_fallback: semantic_fallback_allowed,
                };

                let search_started = Instant::now();
                emit_chat_status(
                    &app_handle,
                    &notebook_id,
                    &conversation_id,
                    &message_id,
                    "semantic_memory_search_start",
                    "Searching semantic-memory preview",
                    None,
                    Some(&model),
                    None,
                    Some("semantic-memory"),
                    Some(&semantic_memory_runtime_config.embedding_model),
                    search_started.elapsed(),
                    Some(semantic_memory_search_timeout),
                    false,
                    None,
                );

                let preview_result = tokio::time::timeout(semantic_memory_search_timeout, async {
                    let _inference =
                        crate::ingestion::native_gates::acquire(&state.gpu_gate, &state.llm_gate)
                            .await?;
                    if crate::commands::sources::semantic_memory_runtime_config_from_state(&state)?
                        != semantic_memory_runtime_config
                    {
                        return Err(GlossError::Embedding(
                            "Semantic embedding configuration changed while waiting; retry retrieval"
                                .into(),
                        ));
                    }
                    semantic_memory_adapter::search_preview(
                        &state.data_dir,
                        &notebook_id,
                        links,
                        &all_sources,
                        preview_request,
                        Some(semantic_memory_runtime_config.clone()),
                        semantic_embedding_metadata,
                    )
                    .await
                })
                .await;

                match preview_result {
                    Err(_) if semantic_fallback_allowed => {
                        let reason = "search-timeout".to_string();
                        emit_chat_status(
                            &app_handle,
                            &notebook_id,
                            &conversation_id,
                            &message_id,
                            "semantic_memory_search_timeout",
                            "semantic-memory preview timed out",
                            None,
                            Some(&model),
                            None,
                            Some("semantic-memory"),
                            Some(&semantic_memory_runtime_config.embedding_model),
                            search_started.elapsed(),
                            Some(semantic_memory_search_timeout),
                            false,
                            Some(&reason),
                        );
                        emit_chat_status(
                            &app_handle,
                            &notebook_id,
                            &conversation_id,
                            &message_id,
                            "semantic_memory_search_fallback",
                            "Falling back to Gloss local retrieval",
                            None,
                            Some(&model),
                            None,
                            Some("gloss-local"),
                            Some(&reason),
                            search_started.elapsed(),
                            Some(semantic_memory_search_timeout),
                            true,
                            None,
                        );
                        retrieval_fallback_reason = Some(reason);
                        force_gloss_local_retrieval = true;
                        retrieval_degradation_markers
                            .push("semantic-memory-preview-timeout-fallback".to_string());
                        record_chat_attempt_trace(
                            &attempt_trace,
                            &trace_data_dir,
                            "semantic_memory_search_fallback",
                            Some(search_started.elapsed()),
                            retrieval_fallback_reason.as_deref(),
                            None,
                            |_| {},
                        );
                        tracing::warn!(
                        timeout_ms = semantic_memory_search_timeout.as_millis(),
                        "semantic-memory preview retrieval timed out; explicit fallback enabled"
                    );
                        None
                    }
                    Err(_) => {
                        let error = format!(
                            "search-timeout: semantic-memory preview timed out after {} ms",
                            semantic_memory_search_timeout.as_millis()
                        );
                        emit_chat_status(
                            &app_handle,
                            &notebook_id,
                            &conversation_id,
                            &message_id,
                            "semantic_memory_search_timeout",
                            "semantic-memory preview timed out",
                            None,
                            Some(&model),
                            None,
                            Some("semantic-memory"),
                            Some(&semantic_memory_runtime_config.embedding_model),
                            search_started.elapsed(),
                            Some(semantic_memory_search_timeout),
                            false,
                            Some(&error),
                        );
                        emit_chat_error(
                            &app_handle,
                            &notebook_id,
                            &conversation_id,
                            &message_id,
                            &error,
                        );
                        record_chat_attempt_trace(
                            &attempt_trace,
                            &trace_data_dir,
                            "semantic_memory_search_timeout",
                            Some(search_started.elapsed()),
                            Some("semantic-memory preview timed out and fallback is disabled"),
                            Some(&error),
                            |_| {},
                        );
                        return Err(GlossError::Search(error));
                    }
                    Ok(Ok(response)) => {
                        if let Some(reason) = response.fallback_reason.clone() {
                            retrieval_fallback_reason =
                                Some(bounded_semantic_fallback_reason(&reason).to_string());
                        }
                        for marker in response.degradation_markers.clone() {
                            if !retrieval_degradation_markers.contains(&marker) {
                                retrieval_degradation_markers.push(marker);
                            }
                        }
                        semantic_memory_receipt =
                            response.provenance.get("semantic_memory_receipt").cloned();
                        retrieval_mode = MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string();
                        retrieval_backend_used = MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string();
                        tracing::info!(
                            receipt_id = %response.receipt_id,
                            candidates = response.candidates.len(),
                            degraded = response.degraded,
                            "semantic-memory preview retrieval completed"
                        );
                        let semantic_results = response
                            .candidates
                            .iter()
                            .map(|candidate| RetrievalResult {
                                chunk_id: Some(candidate.chunk_id.clone()),
                                source_id: candidate.source_id.clone(),
                                title: candidate.source_title.clone(),
                                content: candidate.content.clone(),
                                score: candidate.score,
                                engine: MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string(),
                            })
                            .collect::<Vec<_>>();
                        let semantic_candidate_count = semantic_results.len();
                        let mut semantic_fallback_chain = Vec::new();
                        if response
                            .degradation_markers
                            .iter()
                            .any(|marker| marker == "semantic-memory-backpointer-filtered")
                        {
                            semantic_fallback_chain
                                .push(RetrievalReasonCode::SemanticMemoryLinksDegraded);
                        }
                        if response
                            .degradation_markers
                            .iter()
                            .any(|marker| marker == "source-scope-partial-invalid")
                        {
                            semantic_fallback_chain.push(RetrievalReasonCode::NoRetrievalContext);
                        }
                        if semantic_candidate_count == 0 {
                            semantic_fallback_chain.push(RetrievalReasonCode::NoRetrievalContext);
                        }
                        let semantic_summary = if semantic_candidate_count == 0 {
                            "semantic-memory returned no mapped candidates".to_string()
                        } else {
                            format!(
                                "semantic-memory retrieval used {} mapped candidate(s).",
                                semantic_candidate_count
                            )
                        };
                        let semantic_outcome = RetrievalOutcome {
                            mode: RetrievalMode::SemanticMemory,
                            results: semantic_results,
                            engines: vec![RetrievalEngineStatus {
                                engine: MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string(),
                                attempted: true,
                                available: true,
                                contributed: semantic_candidate_count > 0,
                                candidate_count: semantic_candidate_count,
                                elapsed_ms: search_started.elapsed().as_millis(),
                                reason_code: if semantic_candidate_count == 0 {
                                    Some(RetrievalReasonCode::NoRetrievalContext)
                                } else {
                                    None
                                },
                                detail: Some("semantic-memory backend search result".to_string()),
                            }],
                            coverage: RetrievalCoverage {
                                selected_sources: resolved_scope.source_count(),
                                semantic_links_total: scoped_link_count,
                                semantic_links_healthy: healthy_scoped_links,
                                semantic_links_degraded: scoped_degraded_link_count,
                                ..Default::default()
                            },
                            degraded: response.degraded || semantic_candidate_count == 0,
                            fallback_chain: semantic_fallback_chain,
                            user_visible_summary: semantic_summary.clone(),
                            trace_ref: response.receipt_id.clone(),
                        };
                        record_chat_attempt_trace(
                            &attempt_trace,
                            &trace_data_dir,
                            "retrieval_outcome",
                            Some(search_started.elapsed()),
                            Some(&semantic_summary),
                            None,
                            |trace| {
                                trace.retrieval_trace_ref = Some(response.receipt_id.clone());
                                trace.retrieval_outcome = Some(semantic_outcome.clone());
                            },
                        );
                        retrieval_outcome_for_evidence = Some(semantic_outcome);
                        let context = response
                            .candidates
                            .into_iter()
                            .map(|candidate| ContextPassage {
                                source_id: candidate.source_id,
                                chunk_id: Some(candidate.chunk_id),
                                title: candidate
                                    .source_title
                                    .unwrap_or_else(|| "Untitled source".to_string()),
                                content: candidate.content,
                                evidence_class: "semantic_memory".to_string(),
                            })
                            .collect::<Vec<_>>();
                        if context.is_empty() {
                            retrieval_fallback_reason
                                .get_or_insert_with(|| "no-candidates".to_string());
                            if !retrieval_degradation_markers
                                .iter()
                                .any(|marker| marker == "semantic-memory-empty-context")
                            {
                                retrieval_degradation_markers
                                    .push("semantic-memory-empty-context".to_string());
                            }
                            if semantic_fallback_allowed {
                                retrieval_degradation_markers
                                    .push("semantic-memory-empty-context-fallback".to_string());
                                retrieval_backend_used = MEMORY_BACKEND_GLOSS_LOCAL.to_string();
                                force_gloss_local_retrieval = true;
                                emit_chat_status(
                                    &app_handle,
                                    &notebook_id,
                                    &conversation_id,
                                    &message_id,
                                    "semantic_memory_search_fallback",
                                    "Falling back to Gloss local retrieval",
                                    None,
                                    Some(&model),
                                    None,
                                    Some("gloss-local"),
                                    retrieval_fallback_reason.as_deref(),
                                    search_started.elapsed(),
                                    Some(semantic_memory_search_timeout),
                                    true,
                                    None,
                                );
                                record_chat_attempt_trace(
                                    &attempt_trace,
                                    &trace_data_dir,
                                    "semantic_memory_search_fallback",
                                    Some(search_started.elapsed()),
                                    retrieval_fallback_reason.as_deref(),
                                    None,
                                    |_| {},
                                );
                                None
                            } else {
                                let error = "no-candidates: semantic-memory returned no mapped candidates for the selected source scope".to_string();
                                emit_chat_status(
                                    &app_handle,
                                    &notebook_id,
                                    &conversation_id,
                                    &message_id,
                                    "semantic_memory_search_error",
                                    "semantic-memory returned no mapped candidates",
                                    None,
                                    Some(&model),
                                    None,
                                    Some("semantic-memory"),
                                    None,
                                    search_started.elapsed(),
                                    Some(semantic_memory_search_timeout),
                                    false,
                                    Some(&error),
                                );
                                emit_chat_error(
                                    &app_handle,
                                    &notebook_id,
                                    &conversation_id,
                                    &message_id,
                                    &error,
                                );
                                record_chat_attempt_trace(
                                &attempt_trace,
                                &trace_data_dir,
                                "semantic_memory_search_error",
                                Some(search_started.elapsed()),
                                Some("strict semantic-memory mode failed because no candidates were mapped"),
                                Some(&error),
                                |_| {},
                            );
                                return Err(GlossError::Search(error));
                            }
                        } else {
                            Some(context)
                        }
                    }
                    Ok(Err(err)) if semantic_fallback_allowed => {
                        let reason = "projection-failed".to_string();
                        emit_chat_status(
                            &app_handle,
                            &notebook_id,
                            &conversation_id,
                            &message_id,
                            "semantic_memory_search_error",
                            "semantic-memory preview failed",
                            None,
                            Some(&model),
                            None,
                            Some("semantic-memory"),
                            Some(&semantic_memory_runtime_config.embedding_model),
                            search_started.elapsed(),
                            Some(semantic_memory_search_timeout),
                            false,
                            Some(&reason),
                        );
                        emit_chat_status(
                            &app_handle,
                            &notebook_id,
                            &conversation_id,
                            &message_id,
                            "semantic_memory_search_fallback",
                            "Falling back to Gloss local retrieval",
                            None,
                            Some(&model),
                            None,
                            Some("gloss-local"),
                            Some(&reason),
                            search_started.elapsed(),
                            Some(semantic_memory_search_timeout),
                            true,
                            None,
                        );
                        retrieval_fallback_reason = Some(reason);
                        force_gloss_local_retrieval = true;
                        retrieval_degradation_markers
                            .push("semantic-memory-preview-fallback".to_string());
                        record_chat_attempt_trace(
                            &attempt_trace,
                            &trace_data_dir,
                            "semantic_memory_search_fallback",
                            Some(search_started.elapsed()),
                            retrieval_fallback_reason.as_deref(),
                            None,
                            |_| {},
                        );
                        tracing::warn!(
                            error = %err,
                            "semantic-memory preview retrieval failed; explicit fallback enabled"
                        );
                        None
                    }
                    Ok(Err(err)) => {
                        let error = format!("projection-failed: {err}");
                        emit_chat_status(
                            &app_handle,
                            &notebook_id,
                            &conversation_id,
                            &message_id,
                            "semantic_memory_search_error",
                            "semantic-memory preview failed",
                            None,
                            Some(&model),
                            None,
                            Some("semantic-memory"),
                            Some(&semantic_memory_runtime_config.embedding_model),
                            search_started.elapsed(),
                            Some(semantic_memory_search_timeout),
                            false,
                            Some(&error),
                        );
                        emit_chat_error(
                            &app_handle,
                            &notebook_id,
                            &conversation_id,
                            &message_id,
                            &error,
                        );
                        record_chat_attempt_trace(
                            &attempt_trace,
                            &trace_data_dir,
                            "semantic_memory_search_error",
                            Some(search_started.elapsed()),
                            Some("semantic-memory preview failed and fallback is disabled"),
                            Some(&error),
                            |_| {},
                        );
                        return Err(err);
                    }
                }
            }
        }
        #[cfg(not(feature = "semantic-memory-backend"))]
        {
            if semantic_fallback_allowed {
                let reason = "semantic-memory-not-compiled".to_string();
                emit_chat_status(
                    &app_handle,
                    &notebook_id,
                    &conversation_id,
                    &message_id,
                    "semantic_memory_search_fallback",
                    "Falling back to Gloss local retrieval",
                    None,
                    Some(&model),
                    None,
                    Some("gloss-local"),
                    Some(&reason),
                    Duration::ZERO,
                    None,
                    true,
                    None,
                );
                retrieval_fallback_reason = Some(reason);
                force_gloss_local_retrieval = true;
                retrieval_degradation_markers
                    .push("semantic-memory-feature-disabled-fallback".to_string());
                record_chat_attempt_trace(
                    &attempt_trace,
                    &trace_data_dir,
                    "semantic_memory_search_fallback",
                    Some(Duration::ZERO),
                    retrieval_fallback_reason.as_deref(),
                    None,
                    |_| {},
                );
                tracing::warn!(
                        "semantic-memory preview selected but feature is not enabled; explicit fallback enabled"
                    );
                None
            } else {
                let error =
                    "semantic-memory-not-compiled: semantic-memory-backend feature is not enabled"
                        .to_string();
                emit_chat_status(
                    &app_handle,
                    &notebook_id,
                    &conversation_id,
                    &message_id,
                    "semantic_memory_search_error",
                    "semantic-memory preview is unavailable",
                    None,
                    Some(&model),
                    None,
                    Some("semantic-memory"),
                    None,
                    Duration::ZERO,
                    None,
                    false,
                    Some(&error),
                );
                emit_chat_error(
                    &app_handle,
                    &notebook_id,
                    &conversation_id,
                    &message_id,
                    &error,
                );
                record_chat_attempt_trace(
                    &attempt_trace,
                    &trace_data_dir,
                    "semantic_memory_search_error",
                    Some(Duration::ZERO),
                    Some("semantic-memory preview feature is unavailable and fallback is disabled"),
                    Some(&error),
                    |_| {},
                );
                return Err(GlossError::Config(error));
            }
        }
    } else {
        None
    };

    // 4. Hybrid search with multi-tier fallback
    let source_context: Vec<ContextPassage> = if let Some(context) = semantic_preview_context {
        context
    } else if resolved_scope.is_none() {
        Vec::new()
    } else {
        // GlossLocalMemoryBackend::new — gloss-local retrieval path (ranked before raw-content-text-fallback)
        let mut local_outcome = {
            let app_handle = app_handle.clone();
            let notebook_id = notebook_id.clone();
            let query = query.clone();
            let resolved_scope = resolved_scope.clone();
            let retrieval_receipt_id = retrieval_receipt_id.clone();

            tokio::task::spawn_blocking(move || {
                let state = app_handle.state::<AppState>();
                state.local_retrieval_outcome(
                    &notebook_id,
                    &query,
                    &resolved_scope,
                    top_k,
                    retrieval_receipt_id,
                )
            })
            .await
            .map_err(|err| {
                GlossError::Other(format!("local retrieval execution failed: {err}"))
            })??
        };
        if force_gloss_local_retrieval {
            local_outcome.degraded = true;
            if !local_outcome
                .fallback_chain
                .contains(&RetrievalReasonCode::SemanticMemoryFeatureDisabled)
            {
                local_outcome
                    .fallback_chain
                    .push(RetrievalReasonCode::SemanticMemoryFeatureDisabled);
            }
            local_outcome.user_visible_summary = format!(
                "{} semantic-memory preview fell back to local retrieval.",
                local_outcome.user_visible_summary
            );
        }
        if !local_outcome.fallback_chain.is_empty() {
            for reason in &local_outcome.fallback_chain {
                let marker = reason.as_str().to_string();
                if !retrieval_degradation_markers.contains(&marker) {
                    retrieval_degradation_markers.push(marker);
                }
            }
        }
        if local_outcome.degraded && retrieval_fallback_reason.is_none() {
            retrieval_fallback_reason = Some(local_outcome.user_visible_summary.clone());
        }
        retrieval_mode = local_outcome.mode.as_str().to_string();
        retrieval_backend_used = match local_outcome.mode {
            RetrievalMode::HybridRrf => "native-hybrid",
            RetrievalMode::DenseOnly => "native-dense",
            RetrievalMode::Bm25Only => MEMORY_BACKEND_GLOSS_LOCAL,
            _ => MEMORY_BACKEND_GLOSS_LOCAL,
        }
        .to_string();
        if !local_outcome.results.is_empty() {
            let unique_source_ids: Vec<String> = local_outcome
                .results
                .iter()
                .map(|r| r.source_id.clone())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();

            let title_map: HashMap<String, String> =
                state.with_notebook_db(&notebook_id, |db| {
                    let mut map: HashMap<String, String> = HashMap::new();
                    for sid in &unique_source_ids {
                        if let Ok(source) = db.get_source(sid) {
                            map.insert(sid.clone(), source.title);
                        }
                    }
                    Ok(map)
                })?;

            local_outcome
                .results
                .iter_mut()
                .for_each(|result| result.title = title_map.get(&result.source_id).cloned());
            record_chat_attempt_trace(
                &attempt_trace,
                &trace_data_dir,
                "retrieval_outcome",
                Some(Duration::ZERO),
                Some(&local_outcome.user_visible_summary),
                None,
                |trace| {
                    trace.retrieval_trace_ref = Some(local_outcome.trace_ref.clone());
                    trace.retrieval_outcome = Some(local_outcome.clone());
                },
            );
            retrieval_outcome_for_evidence = Some(local_outcome.clone());
            // Passages arrive score-ordered; stop adding once the total
            // character budget is reached so the prompt stays inside the
            // model context window.
            let mut passages = Vec::with_capacity(local_outcome.results.len());
            let mut total_chars = 0usize;
            for result in &local_outcome.results {
                if total_chars >= MAX_CONTEXT_CHARS_TOTAL && !passages.is_empty() {
                    tracing::info!(
                        included = passages.len(),
                        dropped = local_outcome.results.len() - passages.len(),
                        "Context char budget reached; dropping lowest-ranked passages"
                    );
                    break;
                }
                total_chars += result.content.len();
                passages.push(ContextPassage {
                    source_id: result.source_id.clone(),
                    chunk_id: result.chunk_id.clone(),
                    title: result
                        .title
                        .clone()
                        .unwrap_or_else(|| result.source_id.clone()),
                    content: result.content.clone(),
                    evidence_class: local_outcome.mode.as_str().to_string(),
                });
            }
            passages
        } else {
            // raw-content-text-fallback — indexed retrieval produced no proof-grade context;
            // fallback to Studio-style grounding using ready-source chunks/content text.
            local_outcome.degraded = true;
            retrieval_fallback_reason.get_or_insert_with(|| {
                "indexed retrieval produced no proof-grade context; using source-order fallback"
                    .to_string()
            });
            tracing::info!(
                reason = %local_outcome.user_visible_summary,
                "Indexed retrieval unavailable; falling back to source-order grounding"
            );
            record_chat_attempt_trace(
                &attempt_trace,
                &trace_data_dir,
                "retrieval_outcome",
                Some(Duration::ZERO),
                Some(&local_outcome.user_visible_summary),
                None,
                |trace| {
                    trace.retrieval_trace_ref = Some(local_outcome.trace_ref.clone());
                    trace.retrieval_outcome = Some(local_outcome.clone());
                },
            );
            retrieval_outcome_for_evidence = Some(local_outcome.clone());

            // Build Studio-style snippets from ready sources as grounding fallback.
            let fallback_chunks_by_source: Vec<(String, Vec<Chunk>)> = state
                .with_notebook_db(&notebook_id, |db| {
                    let mut out = Vec::new();
                    for source in resolved_scope.manifest_sources() {
                        let chunks = db.get_chunks_for_source(&source.id)?;
                        out.push((source.id.clone(), chunks));
                    }
                    Ok(out)
                })
                .unwrap_or_default();

            let manifest_sources = resolved_scope.manifest_sources().to_vec();
            let effective_ids_list: Vec<String> = resolved_scope.source_ids().to_vec();
            let requested_scope = if effective_ids_list.is_empty() {
                None
            } else {
                Some(effective_ids_list.as_slice())
            };
            let top_k_val = hybrid_search::compute_top_k(resolved_scope.source_count());

            match build_snippets(
                &manifest_sources,
                &fallback_chunks_by_source,
                requested_scope,
                top_k_val,
                top_k_val,
            ) {
                Ok((_, snippets)) => snippets
                    .iter()
                    .map(|snippet| ContextPassage {
                        source_id: snippet.source_id.clone(),
                        chunk_id: snippet.chunk_id.clone(),
                        title: snippet.source_title.clone(),
                        content: snippet.text.clone(),
                        evidence_class: "source-order-fallback".to_string(),
                    })
                    .collect::<Vec<_>>(),
                Err(err) => {
                    tracing::warn!(
                        "Fallback snippet build failed: {}; returning empty context",
                        err
                    );
                    Vec::new()
                }
            }
        }
    };

    tracing::info!(
        context_passages = source_context.len(),
        manifest_sources = resolved_scope.manifest_sources().len(),
        context_chars = source_context
            .iter()
            .map(|p| p.content.len())
            .sum::<usize>(),
        "RAG context assembled for chat"
    );

    if source_context
        .iter()
        .any(|passage| passage.chunk_id.is_none())
    {
        retrieval_degradation_markers.push("unanchored-context-passages".to_string());
    }
    let semantic_memory_receipt_id = semantic_memory_receipt
        .as_ref()
        .and_then(|receipt| receipt.get("receipt_id"))
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let candidate_backend = semantic_memory_receipt
        .as_ref()
        .and_then(|receipt| receipt.get("candidate_backend"))
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let turbo_quant_generation_id = semantic_memory_receipt
        .as_ref()
        .and_then(|receipt| receipt.get("artifact_generation_id"))
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let vector_artifact_manifest_digest = semantic_memory_receipt
        .as_ref()
        .and_then(|receipt| receipt.get("vector_artifact_manifest_digest"))
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let exact_rerank = semantic_memory_receipt
        .as_ref()
        .and_then(|receipt| receipt.get("exact_rerank"))
        .and_then(|value| value.as_bool());
    let exact_rerank_count = semantic_memory_receipt
        .as_ref()
        .and_then(|receipt| receipt.get("exact_rerank_count"))
        .and_then(|value| value.as_u64())
        .and_then(|value| usize::try_from(value).ok());
    let approximate_candidate_count = semantic_memory_receipt
        .as_ref()
        .and_then(|receipt| receipt.get("approximate_candidate_count"))
        .and_then(|value| value.as_u64())
        .and_then(|value| usize::try_from(value).ok());
    let semantic_memory_fallback_reason = semantic_memory_receipt
        .as_ref()
        .and_then(|receipt| {
            receipt
                .get("fallback_reason")
                .or_else(|| receipt.get("fallback"))
        })
        .and_then(|value| value.as_str())
        .map(bounded_semantic_fallback_reason)
        .map(str::to_string);
    let retrieval_fallback_reason_code = retrieval_outcome_for_evidence
        .as_ref()
        .and_then(|outcome| outcome.fallback_chain.first().cloned())
        .or_else(|| {
            retrieval_fallback_reason
                .as_deref()
                .and_then(retrieval_reason_code_from_text)
        })
        .or_else(|| {
            semantic_memory_fallback_reason
                .as_deref()
                .and_then(retrieval_reason_code_from_text)
        })
        .or_else(|| {
            retrieval_degradation_markers
                .iter()
                .find_map(|marker| retrieval_reason_code_from_text(marker))
        });
    let source_scope_integrity = {
        let requested_ids_valid = invalid_source_ids.is_empty();
        let effective_ids_match_allowed_set = effective_source_ids
            .iter()
            .all(|source_id| resolved_scope.allows(source_id));
        let no_out_of_scope_context = source_context
            .iter()
            .all(|passage| resolved_scope.allows(&passage.source_id));
        let no_unanchored_context = source_context
            .iter()
            .all(|passage| passage.chunk_id.is_some());
        let fallback_class_allowed = retrieval_fallback_reason.is_none()
            || local_indexed_fallback_preserves_scope(
                &retrieval_backend_requested,
                retrieval_outcome_for_evidence.as_ref(),
                &source_context,
            );
        let projection_links_preserved = !retrieval_degradation_markers.iter().any(|marker| {
            marker.contains("semantic_memory")
                || marker.contains("semantic-memory")
                || marker.contains("unanchored")
        });
        let preserved = requested_ids_valid
            && effective_ids_match_allowed_set
            && no_out_of_scope_context
            && no_unanchored_context
            && fallback_class_allowed
            && projection_links_preserved;
        SourceScopeIntegrityV1 {
            requested_ids_valid,
            effective_ids_match_allowed_set,
            no_out_of_scope_context,
            no_unanchored_context,
            fallback_class_allowed,
            projection_links_preserved,
            preserved,
        }
    };

    let (native_dense_ready, native_index_status) =
        native_dense_evidence(retrieval_outcome_for_evidence.as_ref());
    let retrieval_capability_decision = RetrievalCapabilityDecisionV1 {
        requested_backend: retrieval_backend_requested.clone(),
        effective_backend: retrieval_backend_used.clone(),
        decision_reason: retrieval_fallback_reason.clone(),
        decision_reason_code: retrieval_fallback_reason_code.clone(),
        build_feature_available: cfg!(feature = "semantic-memory-backend"),
        runtime_enabled: semantic_preview_gate_open,
        projection_ready: !retrieval_degradation_markers.iter().any(|marker| {
            marker == RetrievalReasonCode::SemanticMemoryLinksMissing.as_str()
                || marker == RetrievalReasonCode::SemanticMemoryLinksDegraded.as_str()
        }),
        dense_ready: native_dense_ready,
        fallback_allowed: semantic_fallback_allowed,
        degraded: retrieval_fallback_reason.is_some() || !retrieval_degradation_markers.is_empty(),
    };
    let semantic_memory_projection_truth = state
        .with_notebook_db(&notebook_id, |db| {
            db.semantic_memory_projection_summary(&notebook_id, &resolved_scope)
        })
        .ok()
        .and_then(|summary| serde_json::to_value(summary).ok())
        .unwrap_or_else(|| serde_json::json!({"projection_summary_unavailable": true}));
    let semantic_memory_runtime_truth = SemanticMemoryRuntimeTruthV1 {
        schema: "SemanticMemoryRuntimeTruthV1".to_string(),
        receipt_id: uuid::Uuid::new_v4().to_string(),
        build: serde_json::json!({
            "semantic_memory_backend_compiled": cfg!(feature = "semantic-memory-backend"),
            "turbo_quant_compiled": cfg!(feature = "semantic-memory-turbo-quant"),
        }),
        settings: serde_json::json!({
            "requested_backend": retrieval_backend_requested,
            "fallback_allowed": semantic_fallback_allowed,
            "runtime_enabled": semantic_preview_gate_open,
        }),
        projection: semantic_memory_projection_truth,
        turbo_quant: serde_json::json!({
            "candidate_backend": candidate_backend.clone(),
            "artifact_generation_id": turbo_quant_generation_id.clone(),
            "vector_artifact_manifest_digest": vector_artifact_manifest_digest.clone(),
            "exact_rerank": exact_rerank,
            "exact_rerank_count": exact_rerank_count,
        }),
        decision: retrieval_capability_decision.clone(),
    };

    let evidence_base = ChatEvidenceDisclosure {
        backend_requested: retrieval_backend_requested.clone(),
        backend_used: retrieval_backend_used.clone(),
        retrieval_mode,
        fallback_used: retrieval_fallback_reason.is_some(),
        fallback_reason: retrieval_fallback_reason,
        fallback_reason_code: retrieval_fallback_reason_code,
        degradation_markers: retrieval_degradation_markers,
        source_scope_mode: source_scope_mode.clone(),
        requested_source_ids: requested_source_ids.clone(),
        selected_source_ids: effective_source_ids.clone(),
        effective_source_ids: effective_source_ids.clone(),
        invalid_source_count: invalid_source_ids.len(),
        invalid_source_ids: invalid_source_ids.clone(),
        excluded_source_ids,
        effective_source_count: effective_source_ids.len(),
        excluded_source_count,
        context_passage_count: source_context.len(),
        citation_valid_count: 0,
        citation_invalid_count: 0,
        citation_anchors: citations::citation_anchors_for_context(&source_context),
        citation_filter_reasons: Vec::new(),
        omitted_candidate_count: 0,
        source_scope_preserved: source_scope_integrity.preserved,
        index_status: if resolved_scope.is_none() {
            "scope-none".to_string()
        } else {
            native_index_status
        },
        link_status: if memory_backend == MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW {
            "semantic-memory-links-checked".to_string()
        } else {
            "gloss-local".to_string()
        },
        receipt_id: retrieval_receipt_id,
        context_digest: source_context_digest(&source_context),
        source_context_digest: source_context_digest(&source_context),
        prompt_digest: None,
        semantic_memory_receipt_id,
        candidate_backend,
        turbo_quant_generation_id,
        vector_artifact_manifest_digest,
        exact_rerank,
        exact_rerank_count,
        approximate_candidate_count,
        semantic_memory_fallback_reason,
        retrieval_outcome: retrieval_outcome_for_evidence,
        retrieval_capability_decision,
        semantic_memory_runtime_truth,
        decoding_settings_receipt: None,
        prompt_receipt: None,
        generation_receipt: None,
        prompt_budget_receipt: None,
    };

    if !state.is_active_notebook_epoch(&notebook_id, request_epoch) {
        let error = "Chat cancelled before provider start because the active notebook changed";
        tracing::info!(
            notebook_id = %notebook_id,
            epoch = request_epoch,
            "Notebook changed during chat preparation; skipping stream start"
        );
        emit_chat_status(
            &app_handle,
            &notebook_id,
            &conversation_id,
            &message_id,
            "cancelled",
            error,
            None,
            Some(&model),
            None,
            None,
            None,
            Duration::ZERO,
            None,
            false,
            Some(error),
        );
        emit_chat_error(
            &app_handle,
            &notebook_id,
            &conversation_id,
            &message_id,
            error,
        );
        record_chat_attempt_trace(
            &attempt_trace,
            &trace_data_dir,
            "cancelled",
            Some(Duration::ZERO),
            Some("Active notebook epoch changed before provider start"),
            Some(error),
            |_| {},
        );
        return Err(GlossError::Other(error.to_string()));
    }

    // For streaming, we spawn an async task
    let msg_id = message_id.clone();
    let conv_id = conversation_id.clone();
    let nb_id = notebook_id.clone();
    let epoch = request_epoch;
    let handle = app_handle.clone();
    let prompt_scope: ResolvedSourceScope = resolved_scope.clone();
    let evidence_for_message = evidence_base.clone();
    let spawned_style = effective_style.clone();
    let spawned_custom_goal = effective_custom_goal.clone();
    let spawned_response_length = effective_response_length.clone();
    let spawned_attempt_id = attempt_id.clone();
    let spawned_user_message_id = user_msg.id.clone();
    let spawned_provider = provider_config.provider_type.as_str().to_string();
    let spawned_operation_receipt_id = evidence_base.receipt_id.clone();
    emit_chat_status(
        &app_handle,
        &notebook_id,
        &conversation_id,
        &message_id,
        "context_built",
        "Context built",
        Some(provider_config.provider_type.as_str()),
        Some(&model),
        None,
        None,
        None,
        Duration::ZERO,
        None,
        false,
        None,
    );
    record_chat_attempt_trace(
        &attempt_trace,
        &trace_data_dir,
        "context_built",
        Some(Duration::ZERO),
        Some("RAG context assembled for provider request"),
        None,
        |_| {},
    );

    let active_chat_attempt = match active_chat_attempt_lease.activate() {
        Ok(attempt) => attempt,
        Err(err) => {
            let error = err.to_string();
            emit_chat_error(
                &app_handle,
                &notebook_id,
                &conversation_id,
                &message_id,
                &error,
            );
            persist_chat_attempt_status(
                &state,
                &notebook_id,
                &conversation_id,
                &attempt_id,
                &message_id,
                Some(&user_msg.id),
                provider_config.provider_type.as_str().into(),
                Some(&model),
                "error",
                Some("lease_activation_failed"),
                Some("lease_activation_failed"),
                Some(&error),
                None,
                true,
            );
            return Err(err);
        }
    };

    // Construct the provider outside any lock (provider_config was extracted above)
    let provider = match providers::build_provider(&provider_config) {
        Ok(provider) => provider,
        Err(err) => {
            state.finish_active_chat_attempt(&notebook_id, &conversation_id, &attempt_id);
            return Err(err);
        }
    };
    let spawned_attempt_trace = Arc::clone(&attempt_trace);
    let spawned_trace_data_dir = trace_data_dir.clone();

    // Create the terminal emitter — guarantees exactly one terminal event
    // (chat:done / chat:error / chat:cancelled) for this chat stream.
    let terminal = ChatTerminalEmitter::new(
        app_handle.clone(),
        &notebook_id,
        &conversation_id,
        &message_id,
    );

    let job = SpawnedChatAttempt {
        handle,
        active_chat_attempt,
        provider,
        terminal,
        notebook_id: nb_id,
        conversation_id: conv_id,
        message_id: msg_id,
        query,
        model,
        history,
        custom_goal: spawned_custom_goal,
        style: spawned_style,
        response_length: spawned_response_length,
        source_scope: prompt_scope,
        source_context,
        model_context_window,
        evidence_for_message,
        epoch,
        attempt_id: spawned_attempt_id,
        user_message_id: spawned_user_message_id,
        provider_name: spawned_provider,
        operation_receipt_id: spawned_operation_receipt_id,
        attempt_trace: spawned_attempt_trace,
        trace_data_dir: spawned_trace_data_dir,
        phase_timeouts: providers::LlmPhaseTimeouts::default(),
    };
    tokio::spawn(job.run_with_panic_boundary());

    Ok(message_id)
}

#[tauri::command]
pub async fn get_suggested_questions(
    notebook_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<String>, GlossError> {
    match load_cached_suggested_questions(&notebook_id, &state) {
        Ok(questions) => Ok(questions),
        Err(e) => {
            tracing::warn!(notebook_id = %notebook_id, error = %e, "Suggested question cache load failed");
            Ok(Vec::new())
        }
    }
}

#[tauri::command]
pub async fn debug_chat_provider_smoke(
    provider_id: String,
    model: String,
    prompt: Option<String>,
    state: State<'_, AppState>,
) -> Result<ChatAttemptTraceV1, GlossError> {
    let provider_type = providers::ProviderType::from_str(provider_id.trim())
        .ok_or_else(|| GlossError::Config(format!("Unknown provider id '{provider_id}'")))?;
    let message_id = uuid::Uuid::new_v4().to_string();
    let attempt_trace = Arc::new(Mutex::new(new_chat_attempt_trace(
        "provider-smoke",
        "provider-smoke",
        &message_id,
        &model,
        None,
        None,
        Some("none".to_string()),
    )));
    let trace_data_dir = state.data_dir.clone();
    record_chat_attempt_trace(
        &attempt_trace,
        &trace_data_dir,
        "provider_smoke_queued",
        Some(Duration::ZERO),
        Some("Provider-only smoke started"),
        None,
        |_| {},
    );

    let config = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        match providers::provider_config_from_db(&app_db, &state.secret_store, provider_type) {
            Ok(config) => config,
            Err(err) => {
                let error = err.to_string();
                record_chat_attempt_trace(
                    &attempt_trace,
                    &trace_data_dir,
                    "provider_config_error",
                    Some(Duration::ZERO),
                    Some("Provider-only smoke could not resolve provider config"),
                    Some(&error),
                    |_| {},
                );
                return Ok(chat_attempt_trace_snapshot(&attempt_trace));
            }
        }
    };
    record_chat_attempt_trace(
        &attempt_trace,
        &trace_data_dir,
        "provider_config_resolved",
        Some(Duration::ZERO),
        Some("Provider-only smoke resolved provider config from provider table"),
        None,
        |trace| {
            trace.provider = config.provider_type.as_str().to_string();
            trace.provider_base_url = Some(config.base_url.clone());
        },
    );

    let provider = providers::build_provider(&config)?;
    let request = ChatRequest {
        model: model.clone(),
        system_prompt: None,
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: prompt.unwrap_or_else(|| "Reply exactly: gloss smoke ok".to_string()),
            images: None,
        }],
        max_tokens: 64,
        temperature: 0.0,
        top_p: None,
        top_k: None,
        min_p: None,
        repeat_penalty: None,
        stream: true,
        num_ctx: None,
    };
    let started = Instant::now();
    record_chat_attempt_trace(
        &attempt_trace,
        &trace_data_dir,
        "provider_request_start",
        Some(started.elapsed()),
        Some("Provider-only smoke request started"),
        None,
        |_| {},
    );

    let smoke_context = providers::LlmExecutionContext::default_with_token(
        tokio_util::sync::CancellationToken::new(),
    )
    .with_attempt_id(chat_attempt_trace_snapshot(&attempt_trace).attempt_id);
    let chat_future = provider.chat(request, smoke_context.clone());
    tokio::pin!(chat_future);
    let mut token_stream = loop {
        if started.elapsed() >= CHAT_PROVIDER_START_TIMEOUT {
            smoke_context.cancellation.cancel();
            let error = "Provider-only smoke did not start streaming before timeout";
            record_chat_attempt_trace(
                &attempt_trace,
                &trace_data_dir,
                "provider_start_timeout",
                Some(started.elapsed()),
                Some(error),
                Some(error),
                |_| {},
            );
            return Ok(chat_attempt_trace_snapshot(&attempt_trace));
        }

        tokio::select! {
            result = &mut chat_future => match result {
                Ok(stream) => break stream,
                Err(err) => {
                    let error = err.to_string();
                    record_chat_attempt_trace(
                        &attempt_trace,
                        &trace_data_dir,
                        "provider_start_error",
                        Some(started.elapsed()),
                        Some("Provider-only smoke failed before stream start"),
                        Some(&error),
                        |_| {},
                    );
                    return Ok(chat_attempt_trace_snapshot(&attempt_trace));
                }
            },
            _ = tokio::time::sleep(Duration::from_millis(250)) => {}
        }
    };

    let first_token_wait_started = Instant::now();
    let mut full_response = String::new();
    let mut first_token_seen = false;
    let mut done_seen = false;
    let mut eof_seen = false;
    let mut last_token_at = Instant::now();
    loop {
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
                    smoke_context.cancellation.cancel();
                    let error = if first_token_seen {
                        "Provider-only smoke stream idle timeout"
                    } else {
                        "Provider-only smoke first-token timeout"
                    };
                    record_chat_attempt_trace(
                        &attempt_trace,
                        &trace_data_dir,
                        if first_token_seen {
                            "stream_idle_timeout"
                        } else {
                            "first_token_timeout"
                        },
                        Some(elapsed),
                        Some(error),
                        Some(error),
                        |_| {},
                    );
                    return Ok(chat_attempt_trace_snapshot(&attempt_trace));
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
                let phase = if smoke_context.is_cancelled() {
                    "cancelled"
                } else {
                    "provider_stream_error"
                };
                record_chat_attempt_trace(
                    &attempt_trace,
                    &trace_data_dir,
                    phase,
                    Some(started.elapsed()),
                    Some("Provider-only smoke stream yielded an error"),
                    Some(&error),
                    |_| {},
                );
                return Ok(chat_attempt_trace_snapshot(&attempt_trace));
            }
        };
        if !first_token_seen {
            first_token_seen = true;
            record_chat_attempt_trace(
                &attempt_trace,
                &trace_data_dir,
                "streaming",
                Some(started.elapsed()),
                Some("Provider-only smoke received first token"),
                None,
                |trace| {
                    trace.first_token_seen = true;
                },
            );
        }
        if !token.is_empty() {
            last_token_at = Instant::now();
        }
        full_response.push_str(&token);
        if done {
            done_seen = true;
            record_chat_attempt_trace(
                &attempt_trace,
                &trace_data_dir,
                "provider_done_frame",
                Some(started.elapsed()),
                Some("Provider-only smoke saw done=true and stopped before HTTP EOF"),
                None,
                |trace| {
                    trace.done_seen = true;
                },
            );
            break;
        }
    }

    if !done_seen {
        let error = "Provider-only smoke stream ended without done";
        record_chat_attempt_trace(
            &attempt_trace,
            &trace_data_dir,
            "incomplete_stream",
            Some(started.elapsed()),
            Some(error),
            Some(error),
            |_| {},
        );
        return Ok(chat_attempt_trace_snapshot(&attempt_trace));
    }
    if full_response.trim().is_empty() {
        let error = "Provider-only smoke completed without response content";
        record_chat_attempt_trace(
            &attempt_trace,
            &trace_data_dir,
            "empty_response",
            Some(started.elapsed()),
            Some(error),
            Some(error),
            |_| {},
        );
        return Ok(chat_attempt_trace_snapshot(&attempt_trace));
    }

    record_chat_attempt_trace(
        &attempt_trace,
        &trace_data_dir,
        "complete",
        Some(started.elapsed()),
        Some(if eof_seen {
            "Provider-only smoke completed with response content after EOF"
        } else {
            "Provider-only smoke completed with response content on provider done frame"
        }),
        None,
        |_| {},
    );
    Ok(chat_attempt_trace_snapshot(&attempt_trace))
}

#[tauri::command]
pub async fn get_last_chat_attempt_trace(
    state: State<'_, AppState>,
) -> Result<Option<ChatAttemptTraceV1>, GlossError> {
    let trace_dir = state.data_dir.join("chat-attempt-traces");
    if !trace_dir.exists() {
        return Ok(None);
    }
    let mut newest: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for entry in std::fs::read_dir(trace_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let modified = entry.metadata()?.modified()?;
        if newest
            .as_ref()
            .map(|(current, _)| modified > *current)
            .unwrap_or(true)
        {
            newest = Some((modified, path));
        }
    }
    let Some((_, path)) = newest else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(path)?;
    Ok(Some(serde_json::from_str(&text)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{RetrievalCoverage, RetrievalEngineStatus, RetrievalResult};
    use async_trait::async_trait;
    use futures::stream;
    use tokio::sync::{oneshot, Mutex as AsyncMutex};

    fn indexed_fallback_fixture() -> (RetrievalOutcome, Vec<ContextPassage>) {
        let outcome = RetrievalOutcome {
            mode: RetrievalMode::Bm25Only,
            results: vec![RetrievalResult {
                chunk_id: Some("chunk-1".to_string()),
                source_id: "selected".to_string(),
                title: None,
                content: "indexed content".to_string(),
                score: 1.0,
                engine: "bm25_fts5".to_string(),
            }],
            engines: vec![
                RetrievalEngineStatus {
                    engine: "bm25_fts5".to_string(),
                    attempted: true,
                    available: true,
                    contributed: true,
                    candidate_count: 1,
                    elapsed_ms: 0,
                    reason_code: None,
                    detail: None,
                },
                RetrievalEngineStatus {
                    engine: "native_dense_hnsw".to_string(),
                    attempted: false,
                    available: false,
                    contributed: false,
                    candidate_count: 0,
                    elapsed_ms: 0,
                    reason_code: Some(RetrievalReasonCode::EmbeddingIndexMetadataStale),
                    detail: None,
                },
            ],
            coverage: RetrievalCoverage {
                total_chunks: 1,
                embedded_chunks: 1,
                dense_coverage_ratio: 1.0,
                ..Default::default()
            },
            degraded: true,
            fallback_chain: vec![RetrievalReasonCode::EmbeddingIndexMetadataStale],
            user_visible_summary: "BM25 with stale dense metadata".to_string(),
            trace_ref: "indexed-fallback-test".to_string(),
        };
        let context = vec![ContextPassage {
            source_id: "selected".to_string(),
            chunk_id: Some("chunk-1".to_string()),
            title: "Selected source".to_string(),
            content: "indexed content".to_string(),
            evidence_class: "bm25_only".to_string(),
        }];
        (outcome, context)
    }

    #[test]
    fn scoped_indexed_bm25_context_survives_optional_dense_degradation() {
        let (outcome, context) = indexed_fallback_fixture();
        assert!(local_indexed_fallback_preserves_scope(
            MEMORY_BACKEND_GLOSS_LOCAL,
            Some(&outcome),
            &context,
        ));
        assert!(!local_indexed_fallback_preserves_scope(
            MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW,
            Some(&outcome),
            &context,
        ));
        assert!(!local_indexed_fallback_preserves_scope(
            MEMORY_BACKEND_GLOSS_LOCAL,
            None,
            &context,
        ));
        assert!(!local_indexed_fallback_preserves_scope(
            MEMORY_BACKEND_GLOSS_LOCAL,
            Some(&outcome),
            &[],
        ));
    }

    #[test]
    fn fallback_integrity_requires_indexed_anchors_and_engine_evidence() {
        let (outcome, context) = indexed_fallback_fixture();
        for field in ["source", "chunk", "unanchored", "class"] {
            let mut changed = context.clone();
            match field {
                "source" => changed[0].source_id = "excluded".to_string(),
                "chunk" => changed[0].chunk_id = Some("other-chunk".to_string()),
                "unanchored" => changed[0].chunk_id = None,
                _ => changed[0].evidence_class = "source-order-fallback".to_string(),
            }
            assert!(
                !local_indexed_fallback_preserves_scope(
                    MEMORY_BACKEND_GLOSS_LOCAL,
                    Some(&outcome),
                    &changed,
                ),
                "{field}"
            );
        }
        for field in [
            "results",
            "engines",
            "available",
            "attempted",
            "contributed",
        ] {
            let mut changed = outcome.clone();
            match field {
                "results" => changed.results.clear(),
                "engines" => changed.engines.clear(),
                "available" => changed.engines[0].available = false,
                "attempted" => changed.engines[0].attempted = false,
                _ => changed.engines[0].contributed = false,
            }
            assert!(
                !local_indexed_fallback_preserves_scope(
                    MEMORY_BACKEND_GLOSS_LOCAL,
                    Some(&changed),
                    &context,
                ),
                "{field}"
            );
        }
    }

    #[test]
    fn only_contributing_native_index_modes_allow_degraded_context() {
        let (mut outcome, mut context) = indexed_fallback_fixture();
        for mode in [
            RetrievalMode::SemanticMemory,
            RetrievalMode::SourceOrderFallback,
            RetrievalMode::RawContentFallback,
            RetrievalMode::Unavailable,
        ] {
            context[0].evidence_class = mode.as_str().to_string();
            outcome.mode = mode;
            assert!(!local_indexed_fallback_preserves_scope(
                MEMORY_BACKEND_GLOSS_LOCAL,
                Some(&outcome),
                &context,
            ));
        }
        for mode in [RetrievalMode::DenseOnly, RetrievalMode::HybridRrf] {
            context[0].evidence_class = mode.as_str().to_string();
            outcome.mode = mode;
            assert!(!local_indexed_fallback_preserves_scope(
                MEMORY_BACKEND_GLOSS_LOCAL,
                Some(&outcome),
                &context,
            ));
            outcome.engines[1].available = true;
            outcome.engines[1].attempted = true;
            outcome.engines[1].contributed = true;
            assert!(local_indexed_fallback_preserves_scope(
                MEMORY_BACKEND_GLOSS_LOCAL,
                Some(&outcome),
                &context,
            ));
            outcome.engines[1].available = false;
        }
    }

    #[test]
    fn dense_readiness_requires_observed_availability_not_stored_inventory() {
        let (mut outcome, _) = indexed_fallback_fixture();
        assert_eq!(
            native_dense_evidence(Some(&outcome)),
            (false, "embedding_index_metadata_stale".to_string())
        );
        outcome.engines[1].reason_code = None;
        assert_eq!(
            native_dense_evidence(Some(&outcome)),
            (false, "native-dense-unavailable".to_string())
        );
        outcome.engines[1].available = true;
        assert_eq!(
            native_dense_evidence(Some(&outcome)),
            (true, "native-dense-enabled".to_string())
        );
        outcome.engines.pop();
        assert_eq!(
            native_dense_evidence(Some(&outcome)),
            (false, "not-observed".to_string())
        );
        assert_eq!(
            native_dense_evidence(None),
            (false, "not-observed".to_string())
        );
    }

    #[test]
    fn decoding_receipt_anthropic_preserves_requested_settings_without_claiming_application() {
        let data_dir = tempfile::tempdir().expect("temporary state directory");
        let state = AppState::initialize_for_test(data_dir.path()).expect("test app state");
        {
            let app_db = state.app_db.lock().expect("app database lock");
            for (key, value) in [
                ("generation_temperature", "0.7"),
                ("generation_top_p", "0.8"),
                ("generation_top_k", "40"),
                ("generation_min_p", "0.05"),
                ("generation_repeat_penalty", "1.1"),
            ] {
                app_db
                    .set_setting(key, value)
                    .expect("saved decoding setting");
            }
        }
        let receipt = effective_decoding_settings(
            &state,
            providers::ProviderType::Anthropic,
            "claude-test-model",
            512,
        )
        .expect("decoding receipt");

        assert_eq!(receipt.provider, "anthropic");
        assert_eq!(receipt.model, "claude-test-model");
        assert_eq!(receipt.effective.max_tokens, 512);
        assert_eq!(receipt.effective.temperature, 1.0);
        assert_eq!(receipt.effective.top_p, None);
        assert_eq!(receipt.effective.top_k, None);
        assert_eq!(receipt.effective.min_p, None);
        assert_eq!(receipt.effective.repeat_penalty, None);
        assert_eq!(
            receipt.requested,
            serde_json::json!({
                "temperature": "0.7", "top_p": "0.8", "top_k": "40",
                "min_p": "0.05", "repeat_penalty": "1.1",
            })
        );
        assert_eq!(
            receipt.unsupported_fields,
            ["temperature", "top_p", "top_k", "min_p", "repeat_penalty"]
        );
        assert!(!receipt.provider_capability.supports_temperature);
        assert!(!receipt.provider_capability.supports_top_p);
        assert!(!receipt.provider_capability.supports_top_k);
        assert!(!receipt.provider_capability.supports_min_p);
        assert!(!receipt.provider_capability.supports_repeat_penalty);
        assert_eq!(
            state
                .app_db
                .lock()
                .expect("app database lock")
                .get_setting("generation_temperature")
                .expect("saved setting"),
            Some("0.7".to_string())
        );
    }

    #[test]
    fn decoding_receipt_anthropic_absent_or_blank_knobs_use_default_without_unsupported_requests() {
        let data_dir = tempfile::tempdir().expect("temporary state directory");
        let state = AppState::initialize_for_test(data_dir.path()).expect("test app state");
        // AppDb migrations seed generation_temperature=0.7. Remove the seeded
        // values explicitly so this fixture actually exercises absent keys.
        state
            .app_db
            .lock()
            .expect("app database lock")
            .conn()
            .execute(
                "DELETE FROM settings WHERE key IN ('generation_temperature', 'generation_top_p', \
                 'generation_top_k', 'generation_min_p', 'generation_repeat_penalty')",
                [],
            )
            .expect("remove seeded decoding settings in fixture");
        for saved in [None, Some(" \t")] {
            if let Some(value) = saved {
                let app_db = state.app_db.lock().expect("app database lock");
                for key in [
                    "generation_temperature",
                    "generation_top_p",
                    "generation_top_k",
                    "generation_min_p",
                    "generation_repeat_penalty",
                ] {
                    app_db
                        .set_setting(key, value)
                        .expect("blank decoding setting");
                }
            }
            let receipt = effective_decoding_settings(
                &state,
                providers::ProviderType::Anthropic,
                "claude-test-model",
                512,
            )
            .expect("decoding receipt");
            assert_eq!(receipt.requested["temperature"], serde_json::json!(saved));
            assert_eq!(receipt.effective.temperature, 1.0);
            assert!(receipt.unsupported_fields.is_empty());
            assert!(!receipt.provider_capability.supports_temperature);
        }
    }

    #[test]
    fn decoding_receipt_other_providers_keep_saved_temperature_and_supported_sampling() {
        let data_dir = tempfile::tempdir().expect("temporary state directory");
        let state = AppState::initialize_for_test(data_dir.path()).expect("test app state");
        {
            let app_db = state.app_db.lock().expect("app database lock");
            for (key, value) in [
                ("generation_temperature", "0.35"),
                ("generation_top_p", "0.8"),
                ("generation_top_k", "40"),
                ("generation_min_p", "0.05"),
                ("generation_repeat_penalty", "1.1"),
            ] {
                app_db
                    .set_setting(key, value)
                    .expect("saved decoding setting");
            }
        }
        for provider in [
            providers::ProviderType::Ollama,
            providers::ProviderType::OpenAI,
            providers::ProviderType::LlamaCpp,
        ] {
            let receipt = effective_decoding_settings(&state, provider, "test-model", 512)
                .expect("decoding receipt");
            assert_eq!(receipt.effective.temperature, 0.35);
            assert_eq!(receipt.effective.top_p, Some(0.8));
            assert!(receipt.provider_capability.supports_temperature);
            assert!(receipt.provider_capability.supports_top_p);
            if matches!(provider, providers::ProviderType::Ollama) {
                assert_eq!(receipt.effective.top_k, Some(40));
                assert_eq!(receipt.effective.min_p, Some(0.05));
                assert_eq!(receipt.effective.repeat_penalty, Some(1.1));
                assert!(receipt.unsupported_fields.is_empty());
            } else {
                assert_eq!(receipt.effective.top_k, None);
                assert_eq!(receipt.effective.min_p, None);
                assert_eq!(receipt.effective.repeat_penalty, None);
                assert_eq!(
                    receipt.unsupported_fields,
                    ["top_k", "min_p", "repeat_penalty"]
                );
            }
        }
    }

    #[derive(Clone, Copy)]
    enum ScriptedLifecycleMode {
        ProviderStartNeverReturns,
        FirstTokenNeverArrives,
        IdleAfterFirstToken,
        ProviderReturnsError,
        Panics,
        CancelDuringStream,
        SuccessfulDone,
    }

    struct ScriptedLifecycleProvider {
        mode: ScriptedLifecycleMode,
        first_token_sent: Option<Arc<AsyncMutex<Option<oneshot::Sender<()>>>>>,
    }

    #[async_trait]
    impl providers::LlmProvider for ScriptedLifecycleProvider {
        async fn list_models(&self) -> Result<Vec<providers::ModelInfo>, GlossError> {
            Ok(Vec::new())
        }

        async fn chat(
            &self,
            _request: ChatRequest,
            _context: providers::LlmExecutionContext,
        ) -> Result<
            std::pin::Pin<Box<dyn futures::Stream<Item = Result<ChatToken, GlossError>> + Send>>,
            GlossError,
        > {
            match self.mode {
                ScriptedLifecycleMode::ProviderStartNeverReturns => {
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
                ScriptedLifecycleMode::FirstTokenNeverArrives => Ok(Box::pin(stream::pending())),
                ScriptedLifecycleMode::IdleAfterFirstToken
                | ScriptedLifecycleMode::CancelDuringStream => {
                    let first_token_sent = self.first_token_sent.clone();
                    Ok(Box::pin(stream::unfold(0_u8, move |step| {
                        let first_token_sent = first_token_sent.clone();
                        async move {
                            match step {
                                0 => {
                                    if let Some(sender) = first_token_sent {
                                        if let Some(sender) = sender.lock().await.take() {
                                            let _ = sender.send(());
                                        }
                                    }
                                    Some((
                                        Ok(ChatToken {
                                            token: "partial".to_string(),
                                            done: false,
                                        }),
                                        1,
                                    ))
                                }
                                _ => futures::future::pending().await,
                            }
                        }
                    })))
                }
                ScriptedLifecycleMode::ProviderReturnsError => Err(GlossError::Other(
                    "scripted lifecycle provider failure".to_string(),
                )),
                ScriptedLifecycleMode::Panics => panic!("scripted lifecycle panic"),
                ScriptedLifecycleMode::SuccessfulDone => Ok(Box::pin(stream::iter([
                    Ok(ChatToken {
                        token: "complete response".to_string(),
                        done: false,
                    }),
                    Ok(ChatToken {
                        token: String::new(),
                        done: true,
                    }),
                ]))),
            }
        }

        async fn health_check(&self) -> Result<bool, GlossError> {
            Ok(true)
        }

        fn provider_type(&self) -> providers::ProviderType {
            providers::ProviderType::Ollama
        }
    }

    #[test]
    fn chat_stream_contract_done_frame_no_eof_terminal_metadata() {
        let decision = provider_done_terminal_decision();

        assert_eq!(decision.terminal_cause, "provider_done_frame");
        assert!(decision.done_frame_seen);
        assert!(!decision.eof_seen);
        assert!(!decision.emit_done_on_current_token);
        assert!(decision.break_stream_loop);
    }

    #[test]
    fn chat_done_frame_without_eof() {
        chat_stream_contract_done_frame_no_eof_terminal_metadata();
    }

    #[test]
    fn stop_chat_response_acknowledges_requests_without_claiming_terminal_state() {
        let response = StopChatResponseV1 {
            cancellation_requested: true,
            attempts: vec![ChatCancellationRequestV1 {
                attempt_id: "attempt-1".to_string(),
                conversation_id: "conversation-1".to_string(),
                message_id: "message-1".to_string(),
            }],
        };

        let value = serde_json::to_value(response).expect("response serializes");
        assert_eq!(value["cancellation_requested"], true);
        assert_eq!(value["attempts"][0]["attempt_id"], "attempt-1");
        assert_eq!(value["attempts"][0]["message_id"], "message-1");
        assert!(value.get("cancelled").is_none());
        assert!(value.get("terminal").is_none());
    }

    #[test]
    fn retrieval_fallback_text_maps_to_typed_reason_codes() {
        assert_eq!(
            retrieval_reason_code_from_text("search-timeout"),
            Some(RetrievalReasonCode::SemanticMemoryTimeout)
        );
        assert_eq!(
            retrieval_reason_code_from_text("projection-required"),
            Some(RetrievalReasonCode::SemanticMemoryLinksMissing)
        );
        assert_eq!(
            retrieval_reason_code_from_text("embedding index native-hnsw is stale"),
            Some(RetrievalReasonCode::EmbeddingIndexMetadataStale)
        );
        assert_eq!(
            retrieval_reason_code_from_text("semantic-memory returned no mapped candidates"),
            Some(RetrievalReasonCode::NoRetrievalContext)
        );
    }

    #[tokio::test]
    async fn spawned_lifecycle_provider_error_emits_one_terminal_event_and_status() {
        let data_dir = tempfile::tempdir().expect("temporary state directory");
        let state = AppState::initialize_for_test(data_dir.path()).expect("test app state");
        let notebook_id = "notebook-1";
        let conversation_id = "conversation-1";
        let notebook_dir = data_dir.path().join("notebooks").join(notebook_id);
        std::fs::create_dir_all(&notebook_dir).expect("test notebook directory");
        {
            let app_db = state.app_db.lock().expect("app database lock");
            app_db
                .create_notebook(
                    notebook_id,
                    "Lifecycle test",
                    &notebook_dir.to_string_lossy(),
                )
                .expect("test notebook registration");
        }
        crate::db::notebook_db::NotebookDb::open(&notebook_dir.join("notebook.db"))
            .expect("test notebook database");
        state
            .with_notebook_db_write(notebook_id, |db| db.create_conversation(conversation_id))
            .expect("test conversation");
        state.set_active_notebook(Some(notebook_id.to_string()), Some(7));
        let active_chat_attempt = state
            .register_active_chat_attempt(notebook_id, conversation_id, "attempt-1", "assistant-1")
            .expect("active test attempt");
        let app = tauri::test::mock_app();
        app.manage(state);
        let trace = Arc::new(Mutex::new(new_chat_attempt_trace(
            notebook_id,
            conversation_id,
            "assistant-1",
            "scripted-model",
            None,
            None,
            Some("none".to_string()),
        )));
        let retrieval_decision = RetrievalCapabilityDecisionV1 {
            requested_backend: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            effective_backend: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            decision_reason: None,
            decision_reason_code: None,
            build_feature_available: cfg!(feature = "semantic-memory-backend"),
            runtime_enabled: false,
            projection_ready: true,
            dense_ready: false,
            fallback_allowed: true,
            degraded: false,
        };
        let evidence_for_message = ChatEvidenceDisclosure {
            backend_requested: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            backend_used: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            retrieval_mode: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            fallback_used: false,
            fallback_reason: None,
            fallback_reason_code: None,
            degradation_markers: Vec::new(),
            source_scope_mode: "none".to_string(),
            requested_source_ids: Vec::new(),
            selected_source_ids: Vec::new(),
            effective_source_ids: Vec::new(),
            invalid_source_ids: Vec::new(),
            excluded_source_ids: Vec::new(),
            invalid_source_count: 0,
            effective_source_count: 0,
            excluded_source_count: 0,
            context_passage_count: 0,
            citation_valid_count: 0,
            citation_invalid_count: 0,
            citation_anchors: Vec::new(),
            citation_filter_reasons: Vec::new(),
            omitted_candidate_count: 0,
            source_scope_preserved: true,
            index_status: "scope-none".to_string(),
            link_status: "gloss-local".to_string(),
            receipt_id: "operation-receipt-1".to_string(),
            context_digest: source_context_digest(&[]),
            source_context_digest: source_context_digest(&[]),
            prompt_digest: None,
            semantic_memory_receipt_id: None,
            candidate_backend: None,
            turbo_quant_generation_id: None,
            vector_artifact_manifest_digest: None,
            exact_rerank: None,
            exact_rerank_count: None,
            approximate_candidate_count: None,
            semantic_memory_fallback_reason: None,
            retrieval_outcome: None,
            retrieval_capability_decision: retrieval_decision.clone(),
            semantic_memory_runtime_truth: SemanticMemoryRuntimeTruthV1 {
                schema: "SemanticMemoryRuntimeTruthV1".to_string(),
                receipt_id: "runtime-truth-1".to_string(),
                build: serde_json::json!({}),
                settings: serde_json::json!({}),
                projection: serde_json::json!({}),
                turbo_quant: serde_json::json!({}),
                decision: retrieval_decision,
            },
            decoding_settings_receipt: None,
            prompt_receipt: None,
            generation_receipt: None,
            prompt_budget_receipt: None,
        };
        let job = SpawnedChatAttempt {
            handle: app.handle().clone(),
            active_chat_attempt,
            provider: Box::new(ScriptedLifecycleProvider {
                mode: ScriptedLifecycleMode::ProviderReturnsError,
                first_token_sent: None,
            }),
            terminal: ChatTerminalEmitter::new(
                app.handle().clone(),
                notebook_id,
                conversation_id,
                "assistant-1",
            ),
            notebook_id: notebook_id.to_string(),
            conversation_id: conversation_id.to_string(),
            message_id: "assistant-1".to_string(),
            query: "hello".to_string(),
            model: "scripted-model".to_string(),
            history: Vec::new(),
            custom_goal: None,
            style: "default".to_string(),
            response_length: "default".to_string(),
            source_scope: SourceScope::None.resolve(&[]),
            source_context: Vec::new(),
            model_context_window: None,
            evidence_for_message,
            epoch: 7,
            attempt_id: "attempt-1".to_string(),
            user_message_id: "user-1".to_string(),
            provider_name: "ollama".to_string(),
            operation_receipt_id: "operation-receipt-1".to_string(),
            attempt_trace: trace,
            trace_data_dir: data_dir.path().to_path_buf(),
            phase_timeouts: providers::LlmPhaseTimeouts {
                provider_start: Duration::from_millis(1),
                first_token: Duration::from_millis(1),
                stream_idle: Duration::from_millis(1),
            },
        };

        job.run_with_panic_boundary().await;

        let state = app.state::<AppState>();
        let events = state.chat_events_since(notebook_id, conversation_id, None);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.kind.as_str(), "done" | "error" | "cancelled"))
                .count(),
            1,
            "the lifecycle must emit exactly one terminal event"
        );
        assert!(events.iter().any(|event| event.kind == "error"));
        assert!(!events.iter().any(|event| event.kind == "done"));
        let (status, phase, terminal): (String, Option<String>, Option<String>) = state
            .with_notebook_db(notebook_id, |db| {
                db.conn()
                    .query_row(
                        "SELECT status, phase, terminal_at FROM chat_attempts WHERE attempt_id = ?1",
                        ["attempt-1"],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(Into::into)
            })
            .expect("terminal chat attempt status");
        assert_eq!(status, "error");
        assert_eq!(phase.as_deref(), Some("stream_error"));
        assert!(terminal.is_some());
    }

    struct SpawnedLifecycleOutcome {
        events: Vec<ChatStreamEventV1>,
        status: String,
        phase: Option<String>,
        terminal_at: Option<String>,
    }

    async fn run_spawned_lifecycle_case(
        mode: ScriptedLifecycleMode,
        persistence_failure: bool,
        cancel_after_first_token: bool,
    ) -> SpawnedLifecycleOutcome {
        let data_dir = tempfile::tempdir().expect("temporary state directory");
        let state = AppState::initialize_for_test(data_dir.path()).expect("test app state");
        let notebook_id = "notebook-1";
        let conversation_id = "conversation-1";
        let message_id = "assistant-1";
        let attempt_id = "attempt-1";
        let notebook_dir = data_dir.path().join("notebooks").join(notebook_id);
        std::fs::create_dir_all(&notebook_dir).expect("test notebook directory");
        {
            let app_db = state.app_db.lock().expect("app database lock");
            app_db
                .create_notebook(
                    notebook_id,
                    "Lifecycle test",
                    &notebook_dir.to_string_lossy(),
                )
                .expect("test notebook registration");
        }
        crate::db::notebook_db::NotebookDb::open(&notebook_dir.join("notebook.db"))
            .expect("test notebook database");
        state
            .with_notebook_db_write(notebook_id, |db| db.create_conversation(conversation_id))
            .expect("test conversation");
        if persistence_failure {
            state
                .with_notebook_db_write(notebook_id, |db| {
                    db.insert_message(&Message {
                        id: message_id.to_string(),
                        conversation_id: conversation_id.to_string(),
                        role: "assistant".to_string(),
                        content: "existing message forces a SQLite primary-key failure".to_string(),
                        citations: None,
                        model_used: None,
                        tokens_prompt: None,
                        tokens_response: None,
                        created_at: String::new(),
                    })
                })
                .expect("deterministic pre-existing assistant message");
        }
        state.set_active_notebook(Some(notebook_id.to_string()), Some(7));
        let active_chat_attempt = state
            .register_active_chat_attempt(notebook_id, conversation_id, attempt_id, message_id)
            .expect("active test attempt");
        let app = tauri::test::mock_app();
        app.manage(state);
        let trace = Arc::new(Mutex::new(new_chat_attempt_trace(
            notebook_id,
            conversation_id,
            message_id,
            "scripted-model",
            None,
            None,
            Some("none".to_string()),
        )));
        let retrieval_decision = RetrievalCapabilityDecisionV1 {
            requested_backend: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            effective_backend: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            decision_reason: None,
            decision_reason_code: None,
            build_feature_available: cfg!(feature = "semantic-memory-backend"),
            runtime_enabled: false,
            projection_ready: true,
            dense_ready: false,
            fallback_allowed: true,
            degraded: false,
        };
        let evidence_for_message = ChatEvidenceDisclosure {
            backend_requested: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            backend_used: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            retrieval_mode: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            fallback_used: false,
            fallback_reason: None,
            fallback_reason_code: None,
            degradation_markers: Vec::new(),
            source_scope_mode: "none".to_string(),
            requested_source_ids: Vec::new(),
            selected_source_ids: Vec::new(),
            effective_source_ids: Vec::new(),
            invalid_source_ids: Vec::new(),
            excluded_source_ids: Vec::new(),
            invalid_source_count: 0,
            effective_source_count: 0,
            excluded_source_count: 0,
            context_passage_count: 0,
            citation_valid_count: 0,
            citation_invalid_count: 0,
            citation_anchors: Vec::new(),
            citation_filter_reasons: Vec::new(),
            omitted_candidate_count: 0,
            source_scope_preserved: true,
            index_status: "scope-none".to_string(),
            link_status: "gloss-local".to_string(),
            receipt_id: "operation-receipt-1".to_string(),
            context_digest: source_context_digest(&[]),
            source_context_digest: source_context_digest(&[]),
            prompt_digest: None,
            semantic_memory_receipt_id: None,
            candidate_backend: None,
            turbo_quant_generation_id: None,
            vector_artifact_manifest_digest: None,
            exact_rerank: None,
            exact_rerank_count: None,
            approximate_candidate_count: None,
            semantic_memory_fallback_reason: None,
            retrieval_outcome: None,
            retrieval_capability_decision: retrieval_decision.clone(),
            semantic_memory_runtime_truth: SemanticMemoryRuntimeTruthV1 {
                schema: "SemanticMemoryRuntimeTruthV1".to_string(),
                receipt_id: "runtime-truth-1".to_string(),
                build: serde_json::json!({}),
                settings: serde_json::json!({}),
                projection: serde_json::json!({}),
                turbo_quant: serde_json::json!({}),
                decision: retrieval_decision,
            },
            decoding_settings_receipt: None,
            prompt_receipt: None,
            generation_receipt: None,
            prompt_budget_receipt: None,
        };
        let (first_token_sent, first_token_received) = if cancel_after_first_token {
            let (sender, receiver) = oneshot::channel();
            (
                Some(Arc::new(AsyncMutex::new(Some(sender)))),
                Some(receiver),
            )
        } else {
            (None, None)
        };
        let job = SpawnedChatAttempt {
            handle: app.handle().clone(),
            active_chat_attempt,
            provider: Box::new(ScriptedLifecycleProvider {
                mode,
                first_token_sent,
            }),
            terminal: ChatTerminalEmitter::new(
                app.handle().clone(),
                notebook_id,
                conversation_id,
                message_id,
            ),
            notebook_id: notebook_id.to_string(),
            conversation_id: conversation_id.to_string(),
            message_id: message_id.to_string(),
            query: "hello".to_string(),
            model: "scripted-model".to_string(),
            history: Vec::new(),
            custom_goal: None,
            style: "default".to_string(),
            response_length: "default".to_string(),
            source_scope: SourceScope::None.resolve(&[]),
            source_context: Vec::new(),
            model_context_window: None,
            evidence_for_message,
            epoch: 7,
            attempt_id: attempt_id.to_string(),
            user_message_id: "user-1".to_string(),
            provider_name: "ollama".to_string(),
            operation_receipt_id: "operation-receipt-1".to_string(),
            attempt_trace: trace,
            trace_data_dir: data_dir.path().to_path_buf(),
            phase_timeouts: providers::LlmPhaseTimeouts {
                provider_start: Duration::from_millis(1),
                first_token: Duration::from_millis(1),
                stream_idle: Duration::from_millis(1),
            },
        };

        if let Some(mut first_token_received) = first_token_received {
            let mut run = Box::pin(job.run_with_panic_boundary());
            tokio::select! {
                _ = &mut run => panic!("lifecycle ended before the scripted first token"),
                received = &mut first_token_received => received.expect("scripted first-token signal"),
            }
            let stop = stop_chat(notebook_id.to_string(), app.state::<AppState>())
                .await
                .expect("user cancellation request");
            assert!(stop.cancellation_requested);
            assert_eq!(stop.attempts.len(), 1);
            run.await;
        } else {
            job.run_with_panic_boundary().await;
        }

        let state = app.state::<AppState>();
        let events = state.chat_events_since(notebook_id, conversation_id, None);
        let (status, phase, terminal_at): (String, Option<String>, Option<String>) = state
            .with_notebook_db(notebook_id, |db| {
                db.conn()
                    .query_row(
                        "SELECT status, phase, terminal_at FROM chat_attempts WHERE attempt_id = ?1",
                        [attempt_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(Into::into)
            })
            .expect("terminal chat attempt status");
        SpawnedLifecycleOutcome {
            events,
            status,
            phase,
            terminal_at,
        }
    }

    fn assert_spawned_terminal_contract(
        outcome: &SpawnedLifecycleOutcome,
        terminal_kind: &str,
        status: &str,
        phase: &str,
    ) {
        assert_eq!(
            outcome
                .events
                .iter()
                .filter(|event| matches!(event.kind.as_str(), "done" | "error" | "cancelled"))
                .count(),
            1,
            "the spawned lifecycle must emit exactly one terminal event"
        );
        assert!(outcome
            .events
            .iter()
            .any(|event| event.kind == terminal_kind));
        assert_eq!(outcome.status, status);
        assert_eq!(outcome.phase.as_deref(), Some(phase));
        assert!(outcome.terminal_at.is_some());
    }

    #[tokio::test]
    async fn spawned_lifecycle_provider_start_timeout_has_terminal_attempt_contract() {
        let outcome = run_spawned_lifecycle_case(
            ScriptedLifecycleMode::ProviderStartNeverReturns,
            false,
            false,
        )
        .await;

        assert_spawned_terminal_contract(&outcome, "error", "error", "stream_error");
        assert!(outcome.events.iter().any(|event| {
            event.kind == "status" && event.payload["phase"] == "provider_start_timeout"
        }));
    }

    #[tokio::test]
    async fn spawned_lifecycle_first_token_timeout_has_terminal_attempt_contract() {
        let outcome =
            run_spawned_lifecycle_case(ScriptedLifecycleMode::FirstTokenNeverArrives, false, false)
                .await;

        assert_spawned_terminal_contract(&outcome, "error", "error", "stream_error");
        assert!(outcome.events.iter().any(|event| {
            event.kind == "status" && event.payload["phase"] == "first_token_timeout"
        }));
    }

    #[tokio::test]
    async fn spawned_lifecycle_stream_idle_timeout_has_terminal_attempt_contract() {
        let outcome =
            run_spawned_lifecycle_case(ScriptedLifecycleMode::IdleAfterFirstToken, false, false)
                .await;

        assert_spawned_terminal_contract(&outcome, "error", "error", "stream_error");
        assert!(outcome.events.iter().any(|event| {
            event.kind == "status" && event.payload["phase"] == "stream_idle_timeout"
        }));
    }

    #[tokio::test]
    async fn spawned_lifecycle_user_cancellation_has_terminal_attempt_contract() {
        let outcome =
            run_spawned_lifecycle_case(ScriptedLifecycleMode::CancelDuringStream, false, true)
                .await;

        assert_spawned_terminal_contract(&outcome, "cancelled", "cancelled", "stream_cancelled");
    }

    #[tokio::test]
    async fn spawned_lifecycle_assistant_persistence_failure_has_terminal_attempt_contract() {
        let outcome =
            run_spawned_lifecycle_case(ScriptedLifecycleMode::SuccessfulDone, true, false).await;

        assert_spawned_terminal_contract(&outcome, "error", "error", "assistant_persist_error");
        assert!(!outcome.events.iter().any(|event| event.kind == "done"));
    }

    #[tokio::test]
    async fn spawned_lifecycle_panic_has_terminal_attempt_contract() {
        let outcome = run_spawned_lifecycle_case(ScriptedLifecycleMode::Panics, false, false).await;

        assert_spawned_terminal_contract(&outcome, "error", "error", "chat_task_panic");
        assert!(outcome.events.iter().any(|event| {
            event.kind == "error"
                && event.payload["error"]
                    .as_str()
                    .is_some_and(|error| error.contains("scripted lifecycle panic"))
        }));
    }
}
