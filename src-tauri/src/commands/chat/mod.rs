#[cfg(feature = "semantic-memory-backend")]
use crate::db::notebook_db::NotebookDb;
use crate::db::notebook_db::{Conversation, Message, Source};
use crate::error::GlossError;
use crate::features;
use crate::jobs;
#[cfg(feature = "semantic-memory-backend")]
use crate::memory::semantic_memory_adapter;
use crate::memory::MemorySearchRequest;
#[cfg(feature = "semantic-memory-backend")]
use crate::memory::{RetrievalCoverage, RetrievalEngineStatus, RetrievalResult};
use crate::memory::{RetrievalMode, RetrievalOutcome, RetrievalReasonCode};
use crate::memory::{MEMORY_BACKEND_GLOSS_LOCAL, MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW};
use crate::providers::{self, ChatMessage, ChatRequest, ChatToken, LlmProvider};
use crate::retrieval::citations;
use crate::retrieval::context::{ContextAssembler, ContextPassage};
use crate::retrieval::hybrid_search;
use crate::retrieval::source_scope::{ResolvedSourceScope, SourceScope};
use crate::state::AppState;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, State};
use tauri_queue::QueueManager;
use tokio::sync::TryAcquireError;

const CHAT_CANCELLED_NOTEBOOK_SWITCH: &str = "__chat_cancelled_notebook_switch__";
const CHAT_PROVIDER_START_TIMEOUT: Duration = Duration::from_secs(180);
const CHAT_FIRST_TOKEN_TIMEOUT: Duration = Duration::from_secs(120);
const CHAT_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(feature = "semantic-memory-backend")]
const SEMANTIC_MEMORY_SEARCH_TIMEOUT: Duration = Duration::from_secs(8);
const DEFAULT_CONTEXT_WINDOW_TOKENS: u32 = 8_192;
const MAX_CONTEXT_WINDOW_TOKENS: u32 = 32_768;

#[derive(Debug, Clone, Serialize)]
struct ChatEvidenceDisclosure {
    backend_requested: String,
    backend_used: String,
    retrieval_mode: String,
    fallback_used: bool,
    fallback_reason: Option<String>,
    degradation_markers: Vec<String>,
    source_scope_mode: String,
    requested_source_ids: Vec<String>,
    selected_source_ids: Vec<String>,
    effective_source_ids: Vec<String>,
    invalid_source_ids: Vec<String>,
    excluded_source_ids: Vec<String>,
    invalid_source_count: usize,
    effective_source_count: usize,
    excluded_source_count: usize,
    context_passage_count: usize,
    citation_valid_count: usize,
    citation_invalid_count: usize,
    citation_anchors: Vec<citations::CitationAnchorV1>,
    citation_filter_reasons: Vec<citations::CitationFilterReasonV1>,
    omitted_candidate_count: usize,
    source_scope_preserved: bool,
    index_status: String,
    link_status: String,
    receipt_id: String,
    context_digest: String,
    source_context_digest: String,
    prompt_digest: Option<String>,
    semantic_memory_receipt_id: Option<String>,
    candidate_backend: Option<String>,
    turbo_quant_generation_id: Option<String>,
    vector_artifact_manifest_digest: Option<String>,
    exact_rerank: Option<bool>,
    exact_rerank_count: Option<usize>,
    approximate_candidate_count: Option<usize>,
    semantic_memory_fallback_reason: Option<String>,
    retrieval_outcome: Option<RetrievalOutcome>,
}

#[derive(Debug, Clone, Serialize)]
struct SourceScopeIntegrityV1 {
    requested_ids_valid: bool,
    effective_ids_match_allowed_set: bool,
    no_out_of_scope_context: bool,
    no_unanchored_context: bool,
    fallback_class_allowed: bool,
    projection_links_preserved: bool,
    preserved: bool,
}

#[derive(Debug, Clone, Serialize)]
struct AssistantMessageEvidence {
    citations: Vec<citations::Citation>,
    evidence: ChatEvidenceDisclosure,
}

#[derive(Debug, Clone, Serialize)]
struct PromptBudgetReceiptV1 {
    model_context_window: u32,
    system_prompt_chars: usize,
    message_count: usize,
    source_passage_count: usize,
    prompt_digest: String,
}

#[derive(Debug, Clone, Serialize)]
struct LlmInvocationReceiptV1 {
    provider: String,
    model: String,
    request_digest: String,
    response_digest: Option<String>,
    error: Option<String>,
}

fn digest_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn source_context_digest(source_context: &[ContextPassage]) -> String {
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

#[derive(Debug, Clone, Serialize)]
struct ChatStatusPayload<'a> {
    notebook_id: &'a str,
    conversation_id: &'a str,
    message_id: &'a str,
    phase: &'a str,
    message: &'a str,
    provider: Option<&'a str>,
    model: Option<&'a str>,
    gate: Option<&'a str>,
    owner: Option<&'a str>,
    owner_detail: Option<&'a str>,
    elapsed_ms: u128,
    timeout_ms: Option<u128>,
    truncated: bool,
    error: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatAttemptTraceEvent {
    pub phase: String,
    pub recorded_at: String,
    pub elapsed_ms: Option<u128>,
    pub detail: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatAttemptTraceV1 {
    pub schema: String,
    pub attempt_id: String,
    pub notebook_id: String,
    pub conversation_id: String,
    pub message_id: String,
    pub model: String,
    pub provider: String,
    pub provider_base_url: Option<String>,
    pub memory_backend: Option<String>,
    pub memory_backend_fallback: Option<bool>,
    pub source_scope_mode: Option<String>,
    pub first_token_seen: bool,
    pub done_seen: bool,
    pub assistant_persisted: bool,
    pub error: Option<String>,
    pub retrieval_trace_ref: Option<String>,
    pub retrieval_outcome: Option<RetrievalOutcome>,
    pub events: Vec<ChatAttemptTraceEvent>,
}

fn new_chat_attempt_trace(
    notebook_id: &str,
    conversation_id: &str,
    message_id: &str,
    model: &str,
    memory_backend: Option<String>,
    memory_backend_fallback: Option<bool>,
    source_scope_mode: Option<String>,
) -> ChatAttemptTraceV1 {
    ChatAttemptTraceV1 {
        schema: "ChatAttemptTraceV1".to_string(),
        attempt_id: uuid::Uuid::new_v4().to_string(),
        notebook_id: notebook_id.to_string(),
        conversation_id: conversation_id.to_string(),
        message_id: message_id.to_string(),
        model: model.to_string(),
        provider: "unknown".to_string(),
        provider_base_url: None,
        memory_backend,
        memory_backend_fallback,
        source_scope_mode,
        first_token_seen: false,
        done_seen: false,
        assistant_persisted: false,
        error: None,
        retrieval_trace_ref: None,
        retrieval_outcome: None,
        events: Vec::new(),
    }
}

fn persist_chat_attempt_trace(
    data_dir: &Path,
    trace: &ChatAttemptTraceV1,
) -> Result<(), GlossError> {
    let trace_dir = data_dir.join("chat-attempt-traces");
    std::fs::create_dir_all(&trace_dir)?;
    let bytes = serde_json::to_vec_pretty(trace)?;
    std::fs::write(trace_dir.join(format!("{}.json", trace.attempt_id)), &bytes)?;
    Ok(())
}

fn record_chat_attempt_trace<F>(
    trace: &Arc<Mutex<ChatAttemptTraceV1>>,
    data_dir: &Path,
    phase: &str,
    elapsed: Option<Duration>,
    detail: Option<&str>,
    error: Option<&str>,
    update: F,
) where
    F: FnOnce(&mut ChatAttemptTraceV1),
{
    let snapshot = {
        let mut guard = trace.lock().unwrap_or_else(|e| e.into_inner());
        update(&mut guard);
        if let Some(error) = error {
            guard.error = Some(error.to_string());
        }
        guard.events.push(ChatAttemptTraceEvent {
            phase: phase.to_string(),
            recorded_at: chrono::Utc::now().to_rfc3339(),
            elapsed_ms: elapsed.map(|elapsed| elapsed.as_millis()),
            detail: detail.map(str::to_string),
            error: error.map(str::to_string),
        });
        guard.clone()
    };

    if let Err(err) = persist_chat_attempt_trace(data_dir, &snapshot) {
        tracing::warn!(error = %err, "Failed to persist chat attempt trace");
    }
}

fn chat_attempt_trace_snapshot(trace: &Arc<Mutex<ChatAttemptTraceV1>>) -> ChatAttemptTraceV1 {
    trace.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[allow(clippy::too_many_arguments)]
fn emit_chat_status(
    handle: &tauri::AppHandle,
    notebook_id: &str,
    conversation_id: &str,
    message_id: &str,
    phase: &str,
    message: &str,
    provider: Option<&str>,
    model: Option<&str>,
    gate: Option<&str>,
    owner: Option<&str>,
    owner_detail: Option<&str>,
    elapsed: Duration,
    timeout: Option<Duration>,
    truncated: bool,
    error: Option<&str>,
) {
    let _ = handle.emit(
        "chat:status",
        ChatStatusPayload {
            notebook_id,
            conversation_id,
            message_id,
            phase,
            message,
            provider,
            model,
            gate,
            owner,
            owner_detail,
            elapsed_ms: elapsed.as_millis(),
            timeout_ms: timeout.map(|timeout| timeout.as_millis()),
            truncated,
            error,
        },
    );
}

fn gate_owner_for(state: &AppState, gate_name: &str) -> Option<crate::state::RuntimeGateOwner> {
    state
        .gate_owners_snapshot()
        .into_iter()
        .find(|owner| owner.gate == gate_name)
}

fn emit_chat_done(
    handle: &tauri::AppHandle,
    notebook_id: &str,
    conversation_id: &str,
    message_id: &str,
) {
    let _ = handle.emit(
        "chat:token",
        serde_json::json!({
            "notebook_id": notebook_id,
            "conversation_id": conversation_id,
            "message_id": message_id,
            "token": "",
            "done": true,
        }),
    );
}

fn emit_chat_error(
    handle: &tauri::AppHandle,
    notebook_id: &str,
    conversation_id: &str,
    message_id: &str,
    error: &str,
) {
    let _ = handle.emit(
        "chat:error",
        serde_json::json!({
            "notebook_id": notebook_id,
            "conversation_id": conversation_id,
            "message_id": message_id,
            "error": error,
        }),
    );
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

#[allow(clippy::too_many_arguments)]
async fn acquire_gate_with_epoch<'a>(
    app_handle: &tauri::AppHandle,
    state: &'a AppState,
    notebook_id: &str,
    conversation_id: &str,
    message_id: &str,
    epoch: u64,
    gate: &'a tokio::sync::Semaphore,
    timeout: Duration,
    gate_name: &str,
) -> Result<Option<tokio::sync::SemaphorePermit<'a>>, GlossError> {
    let started = Instant::now();
    let mut last_wait_status = Instant::now() - Duration::from_secs(2);
    let owner = gate_owner_for(state, gate_name);
    emit_chat_status(
        app_handle,
        notebook_id,
        conversation_id,
        message_id,
        "waiting_gate",
        "Waiting for runtime gate",
        None,
        None,
        Some(gate_name),
        owner.as_ref().map(|owner| owner.owner.as_str()),
        owner.as_ref().map(|owner| owner.detail.as_str()),
        started.elapsed(),
        Some(timeout),
        false,
        None,
    );

    loop {
        if !state.is_active_notebook_epoch(notebook_id, epoch) {
            emit_chat_status(
                app_handle,
                notebook_id,
                conversation_id,
                message_id,
                "cancelled",
                "Chat cancelled by notebook switch",
                None,
                None,
                Some(gate_name),
                None,
                None,
                started.elapsed(),
                Some(timeout),
                false,
                None,
            );
            return Ok(None);
        }

        match gate.try_acquire() {
            Ok(permit) => {
                state.set_gate_owner(gate_name, "chat", message_id);
                emit_chat_status(
                    app_handle,
                    notebook_id,
                    conversation_id,
                    message_id,
                    "gate_acquired",
                    "Runtime gate acquired",
                    None,
                    None,
                    Some(gate_name),
                    Some("chat"),
                    Some(message_id),
                    started.elapsed(),
                    Some(timeout),
                    false,
                    None,
                );
                return Ok(Some(permit));
            }
            Err(TryAcquireError::NoPermits) => {
                if started.elapsed() >= timeout {
                    let error = format!("Timed out waiting for {gate_name}.");
                    let owner = gate_owner_for(state, gate_name);
                    emit_chat_status(
                        app_handle,
                        notebook_id,
                        conversation_id,
                        message_id,
                        "gate_timeout",
                        "Timed out waiting for runtime gate",
                        None,
                        None,
                        Some(gate_name),
                        owner.as_ref().map(|owner| owner.owner.as_str()),
                        owner.as_ref().map(|owner| owner.detail.as_str()),
                        started.elapsed(),
                        Some(timeout),
                        false,
                        Some(&error),
                    );
                    return Err(GlossError::Other(error));
                }
                if last_wait_status.elapsed() >= Duration::from_secs(1) {
                    let owner = gate_owner_for(state, gate_name);
                    emit_chat_status(
                        app_handle,
                        notebook_id,
                        conversation_id,
                        message_id,
                        "waiting_gate",
                        "Waiting for runtime gate",
                        None,
                        None,
                        Some(gate_name),
                        owner.as_ref().map(|owner| owner.owner.as_str()),
                        owner.as_ref().map(|owner| owner.detail.as_str()),
                        started.elapsed(),
                        Some(timeout),
                        false,
                        None,
                    );
                    last_wait_status = Instant::now();
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(TryAcquireError::Closed) => {
                return Err(GlossError::Other(format!("{gate_name} closed")));
            }
        }
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
) -> u32 {
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

    needed.clamp(DEFAULT_CONTEXT_WINDOW_TOKENS.min(model_limit), model_limit)
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
    state.with_notebook_db(&notebook_id, |db| db.create_conversation(&id))?;
    Ok(id)
}

#[tauri::command]
pub async fn delete_conversation(
    notebook_id: String,
    conversation_id: String,
    state: State<'_, AppState>,
) -> Result<(), GlossError> {
    state.with_notebook_db(&notebook_id, |db| db.delete_conversation(&conversation_id))
}

#[tauri::command]
pub async fn stop_chat(notebook_id: String, state: State<'_, AppState>) -> Result<(), GlossError> {
    if state.get_active_notebook_id().as_deref() == Some(notebook_id.as_str()) {
        state.active_epoch.fetch_add(1, Ordering::SeqCst);
        state.bump_user_activity();
    }
    Ok(())
}

#[tauri::command]
pub async fn load_messages(
    notebook_id: String,
    conversation_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<Message>, GlossError> {
    state.with_notebook_db(&notebook_id, |db| db.load_messages(&conversation_id))
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
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
    app_handle: tauri::AppHandle,
) -> Result<String, GlossError> {
    let message_id = message_id
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
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

    let cancelled = jobs::cancel_jobs_matching(&queue, |_job, status| status == "processing");
    if cancelled > 0 {
        tracing::info!(
            cancelled,
            "Cancelled in-flight background jobs for chat preemption"
        );
    }

    // Load history BEFORE inserting user message to avoid duplicate
    let (history, custom_goal, style) = state.with_notebook_db(&notebook_id, |db| {
        let history = db.load_messages(&conversation_id)?;
        let goal = db.get_config("custom_goal")?;
        let style = db
            .get_config("default_style")?
            .unwrap_or_else(|| "default".to_string());
        Ok((history, goal, style))
    })?;

    // Store user message
    let user_msg = Message {
        id: uuid::Uuid::new_v4().to_string(),
        conversation_id: conversation_id.clone(),
        role: "user".to_string(),
        content: query.clone(),
        citations: None,
        model_used: None,
        tokens_prompt: None,
        tokens_response: None,
        created_at: String::new(),
    };
    state.with_notebook_db(&notebook_id, |db| db.insert_message(&user_msg))?;
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
    ) =
        {
            let app_db = state
                .app_db
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            (
                app_db
                    .get_setting("memory_backend")?
                    .unwrap_or_else(|| "gloss-local".to_string()),
                setting_is_enabled(app_db.get_setting("memory_backend_fallback")?),
                semantic_memory_adapter::runtime_config_from_settings(
                    app_db.get_setting("semantic_memory_embedding_provider")?,
                    app_db.get_setting("semantic_memory_embedding_url")?,
                    app_db.get_setting("semantic_memory_embedding_model")?,
                    app_db.get_setting("semantic_memory_embedding_timeout_secs")?,
                    features::turbo_quant_active(&app_db)?,
                    setting_is_enabled(app_db.get_setting(
                        features::SEMANTIC_MEMORY_TURBO_QUANT_REQUIRE_FRESH_ARTIFACTS,
                    )?),
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

                let preview_result = tokio::time::timeout(
                    semantic_memory_search_timeout,
                    semantic_memory_adapter::search_preview(
                        &state.data_dir,
                        &notebook_id,
                        links,
                        &all_sources,
                        preview_request,
                        Some(semantic_memory_runtime_config.clone()),
                    ),
                )
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
        let mut local_outcome = state.local_retrieval_outcome(
            &notebook_id,
            &query,
            &resolved_scope,
            top_k,
            retrieval_receipt_id.clone(),
        )?;
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
                    let mut map = HashMap::new();
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
            local_outcome
                .results
                .iter()
                .map(|result| ContextPassage {
                    source_id: result.source_id.clone(),
                    chunk_id: result.chunk_id.clone(),
                    title: result
                        .title
                        .clone()
                        .unwrap_or_else(|| result.source_id.clone()),
                    content: result.content.clone(),
                    evidence_class: local_outcome.mode.as_str().to_string(),
                })
                .collect()
        } else {
            local_outcome.degraded = true;
            retrieval_fallback_reason.get_or_insert_with(|| {
                "indexed retrieval produced no proof-grade context; answer must disclose missing support"
                    .to_string()
            });
            tracing::info!(
                reason = %local_outcome.user_visible_summary,
                "Indexed retrieval unavailable; continuing without proof-grade context"
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
            retrieval_outcome_for_evidence = Some(local_outcome);
            Vec::new()
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
        let fallback_class_allowed = retrieval_fallback_reason.is_none();
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

    let evidence_base = ChatEvidenceDisclosure {
        backend_requested: retrieval_backend_requested,
        backend_used: retrieval_backend_used,
        retrieval_mode,
        fallback_used: retrieval_fallback_reason.is_some(),
        fallback_reason: retrieval_fallback_reason,
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
        } else if native_dense_possible {
            "native-dense-enabled".to_string()
        } else {
            "fallback".to_string()
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

    // Construct the provider outside any lock (provider_config was extracted above)
    let provider = providers::build_provider(&provider_config);
    let spawned_attempt_trace = Arc::clone(&attempt_trace);
    let spawned_trace_data_dir = trace_data_dir.clone();

    tokio::spawn(async move {
        use tauri::Manager;
        let app_state: tauri::State<'_, AppState> = handle.state();

        // Acquire gates in the same order as the background summary loop:
        // GPU first, then LLM. Keeping one global order prevents lock-order
        // inversion when chat races a summary job.
        let gpu_permit = match acquire_gate_with_epoch(
            &handle,
            &app_state,
            &nb_id,
            &conv_id,
            &msg_id,
            epoch,
            &app_state.gpu_gate,
            Duration::from_secs(120),
            "GPU gate",
        )
        .await
        {
            Ok(Some(permit)) => permit,
            Ok(None) => {
                record_chat_attempt_trace(
                    &spawned_attempt_trace,
                    &spawned_trace_data_dir,
                    "cancelled",
                    Some(Duration::ZERO),
                    Some("GPU gate wait cancelled by notebook switch"),
                    Some("chat cancelled before acquiring GPU gate"),
                    |_| {},
                );
                return;
            }
            Err(e) => {
                tracing::error!(message_id = %msg_id, error = %e, "GPU gate acquisition failed");
                emit_chat_error(&handle, &nb_id, &conv_id, &msg_id, &e.to_string());
                record_chat_attempt_trace(
                    &spawned_attempt_trace,
                    &spawned_trace_data_dir,
                    "gate_error",
                    Some(Duration::ZERO),
                    Some("GPU gate acquisition failed"),
                    Some(&e.to_string()),
                    |_| {},
                );
                return;
            }
        };

        let permit = match acquire_gate_with_epoch(
            &handle,
            &app_state,
            &nb_id,
            &conv_id,
            &msg_id,
            epoch,
            &app_state.llm_gate,
            Duration::from_secs(120),
            "LLM gate",
        )
        .await
        {
            Ok(Some(permit)) => permit,
            Ok(None) => {
                app_state.clear_gate_owner("GPU gate", "chat");
                drop(gpu_permit);
                record_chat_attempt_trace(
                    &spawned_attempt_trace,
                    &spawned_trace_data_dir,
                    "cancelled",
                    Some(Duration::ZERO),
                    Some("LLM gate wait cancelled by notebook switch"),
                    Some("chat cancelled before acquiring LLM gate"),
                    |_| {},
                );
                return;
            }
            Err(e) => {
                app_state.clear_gate_owner("GPU gate", "chat");
                drop(gpu_permit);
                emit_chat_error(&handle, &nb_id, &conv_id, &msg_id, &e.to_string());
                record_chat_attempt_trace(
                    &spawned_attempt_trace,
                    &spawned_trace_data_dir,
                    "gate_error",
                    Some(Duration::ZERO),
                    Some("LLM gate acquisition failed"),
                    Some(&e.to_string()),
                    |_| {},
                );
                return;
            }
        };

        let result = stream_chat_response(
            &handle,
            provider.as_ref(),
            &nb_id,
            epoch,
            &conv_id,
            &msg_id,
            &query,
            &model,
            &history,
            custom_goal.as_deref(),
            &style,
            &prompt_scope,
            &source_context,
            model_context_window,
            &spawned_attempt_trace,
            &spawned_trace_data_dir,
        )
        .await;

        match &result {
            Ok(full_response) => {
                if !app_state.is_active_notebook_epoch(&nb_id, epoch) {
                    emit_chat_done(&handle, &nb_id, &conv_id, &msg_id);
                    app_state.clear_gate_owner("GPU gate", "chat");
                    app_state.clear_gate_owner("LLM gate", "chat");
                    drop(gpu_permit);
                    drop(permit);
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
                let citations_payload = AssistantMessageEvidence {
                    citations: extracted,
                    evidence,
                };
                let citations_json = serde_json::to_string(&citations_payload).ok();

                // Persist assistant message to DB
                let assistant_msg = Message {
                    id: msg_id.clone(),
                    conversation_id: conv_id.clone(),
                    role: "assistant".to_string(),
                    content: full_response.clone(),
                    citations: citations_json,
                    model_used: Some(model.clone()),
                    tokens_prompt: None,
                    tokens_response: None,
                    created_at: String::new(),
                };
                if let Err(e) =
                    app_state.with_notebook_db(&nb_id, |db| db.insert_message(&assistant_msg))
                {
                    tracing::error!(message_id = %msg_id, "Failed to persist assistant message: {}", e);
                    record_chat_attempt_trace(
                        &spawned_attempt_trace,
                        &spawned_trace_data_dir,
                        "assistant_persist_error",
                        Some(Duration::ZERO),
                        Some("Assistant message streaming completed but persistence failed"),
                        Some(&e.to_string()),
                        |_| {},
                    );
                } else {
                    record_chat_attempt_trace(
                        &spawned_attempt_trace,
                        &spawned_trace_data_dir,
                        "assistant_persisted",
                        Some(Duration::ZERO),
                        Some("Assistant message persisted after provider completion"),
                        None,
                        |trace| {
                            trace.assistant_persisted = true;
                        },
                    );
                }
                let _ = handle.emit(
                    "chat:evidence",
                    serde_json::json!({
                        "notebook_id": nb_id,
                        "conversation_id": conv_id,
                        "message_id": msg_id,
                        "citations": citations_payload.citations,
                        "evidence": citations_payload.evidence,
                    }),
                );
            }
            Err(e) => {
                if e.to_string() != CHAT_CANCELLED_NOTEBOOK_SWITCH {
                    let error_text = e.to_string();
                    tracing::error!(message_id = %msg_id, "Chat streaming failed: {}", error_text);
                    emit_chat_status(
                        &handle,
                        &nb_id,
                        &conv_id,
                        &msg_id,
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
                    emit_chat_error(&handle, &nb_id, &conv_id, &msg_id, &error_text);
                    record_chat_attempt_trace(
                        &spawned_attempt_trace,
                        &spawned_trace_data_dir,
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
    });

    Ok(message_id)
}

#[allow(clippy::too_many_arguments)]
async fn stream_chat_response(
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
) -> Result<String, GlossError> {
    use tauri::Manager;

    // Build system prompt with source manifest and authority rules only.
    let system_prompt = ContextAssembler::build_system_prompt(
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

    let history_msgs = ContextAssembler::format_history(history, 10);
    for (role, content) in &history_msgs {
        chat_messages.push(ChatMessage {
            role: role.clone(),
            content: content.clone(),
            images: None,
        });
    }

    let user_turn = ContextAssembler::build_user_turn(query, source_context);
    chat_messages.push(ChatMessage {
        role: "user".to_string(),
        content: user_turn,
        images: None,
    });

    let max_tokens = 2048;
    let num_ctx = compute_dynamic_num_ctx(
        &system_prompt,
        &chat_messages,
        model_context_window,
        max_tokens,
    );
    let request_material = serde_json::json!({
        "system": &system_prompt,
        "messages": &chat_messages,
        "model": model,
        "num_ctx": num_ctx,
        "max_tokens": max_tokens,
    })
    .to_string();
    let request_digest = digest_text(&request_material);
    let prompt_budget_receipt = PromptBudgetReceiptV1 {
        model_context_window: num_ctx,
        system_prompt_chars: request_material.len(),
        message_count: history_msgs.len() + 1,
        source_passage_count: source_context.len(),
        prompt_digest: request_digest.clone(),
    };
    let prompt_budget_detail = serde_json::to_string(&prompt_budget_receipt).ok();
    record_chat_attempt_trace(
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
        temperature: 0.7,
        stream: true,
        num_ctx: Some(num_ctx),
    };

    let state: tauri::State<'_, AppState> = app_handle.state();

    if !state.is_active_notebook_epoch(notebook_id, epoch) {
        record_chat_attempt_trace(
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
    record_chat_attempt_trace(
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
            record_chat_attempt_trace(
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
            record_chat_attempt_trace(
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
                    record_chat_attempt_trace(
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
    record_chat_attempt_trace(
        attempt_trace,
        trace_data_dir,
        "first_token_wait",
        Some(first_token_wait_started.elapsed()),
        Some("Waiting for first provider token"),
        None,
        |_| {},
    );

    let mut full_response = String::new();
    let mut sent_done = false;
    let mut first_token_seen = false;
    let mut last_token_at = Instant::now();

    loop {
        if !state.is_active_notebook_epoch(notebook_id, epoch) {
            record_chat_attempt_trace(
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
                    record_chat_attempt_trace(
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
                record_chat_attempt_trace(
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
            record_chat_attempt_trace(
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
        }

        full_response.push_str(&token);

        if done {
            sent_done = true;
        }

        let _ = app_handle.emit(
            "chat:token",
            serde_json::json!({
                "notebook_id": notebook_id,
                "conversation_id": conversation_id,
                "message_id": message_id,
                "token": token,
                "done": false,
            }),
        );
    }

    if !sent_done {
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
        record_chat_attempt_trace(
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
        record_chat_attempt_trace(
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
    record_chat_attempt_trace(
        attempt_trace,
        trace_data_dir,
        "complete",
        Some(started.elapsed()),
        Some("Provider stream completed cleanly"),
        None,
        |trace| {
            trace.done_seen = true;
        },
    );
    let invocation = LlmInvocationReceiptV1 {
        provider: provider.provider_type().as_str().to_string(),
        model: model.to_string(),
        request_digest,
        response_digest: Some(digest_text(&full_response)),
        error: None,
    };
    let invocation_detail = serde_json::to_string(&invocation).ok();
    record_chat_attempt_trace(
        attempt_trace,
        trace_data_dir,
        "llm_invocation_receipt",
        Some(started.elapsed()),
        invocation_detail.as_deref(),
        None,
        |_| {},
    );
    emit_chat_done(app_handle, notebook_id, conversation_id, message_id);

    tracing::debug!(
        message_id,
        len = full_response.len(),
        "Chat response complete"
    );

    Ok(full_response)
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

    let provider = providers::build_provider(&config);
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

    let mut token_stream =
        match tokio::time::timeout(CHAT_PROVIDER_START_TIMEOUT, provider.chat(request)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(err)) => {
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
            Err(_) => {
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
        };

    let first_token_wait_started = Instant::now();
    let mut full_response = String::new();
    let mut first_token_seen = false;
    let mut done_seen = false;
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
            break;
        };
        let ChatToken { token, done } = match result {
            Ok(token) => token,
            Err(err) => {
                let error = err.to_string();
                record_chat_attempt_trace(
                    &attempt_trace,
                    &trace_data_dir,
                    "provider_stream_error",
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
        Some("Provider-only smoke completed with response content"),
        None,
        |trace| {
            trace.done_seen = true;
        },
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
