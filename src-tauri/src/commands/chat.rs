#[cfg(feature = "semantic-memory-backend")]
use crate::db::notebook_db::NotebookDb;
use crate::db::notebook_db::{Conversation, Message, Source};
use crate::error::GlossError;
use crate::jobs;
use crate::memory::backend::MemorySearchBackend;
use crate::memory::gloss_local::GlossLocalMemoryBackend;
#[cfg(feature = "semantic-memory-backend")]
use crate::memory::semantic_memory_adapter;
use crate::memory::MemorySearchRequest;
use crate::memory::{MEMORY_BACKEND_GLOSS_LOCAL, MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW};
use crate::providers::{self, ChatMessage, ChatRequest, ChatToken, LlmProvider};
use crate::retrieval::citations;
use crate::retrieval::context::{ContextAssembler, ContextPassage};
use crate::retrieval::hybrid_search;
use crate::retrieval::source_scope::{ResolvedSourceScope, SourceScope};
use crate::state::AppState;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, State};
use tauri_queue::QueueManager;
use tokio::sync::TryAcquireError;

/// Maximum characters of source content to inject per source (fallback path).
const MAX_SOURCE_CHARS: usize = 8_000;
/// Maximum total characters of all source context combined (fallback path).
const MAX_TOTAL_CONTEXT_CHARS: usize = 32_000;
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
    omitted_candidate_count: usize,
    source_scope_preserved: bool,
    index_status: String,
    link_status: String,
    receipt_id: String,
    semantic_memory_receipt_id: Option<String>,
    candidate_backend: Option<String>,
    turbo_quant_generation_id: Option<String>,
    vector_artifact_manifest_digest: Option<String>,
    exact_rerank: Option<bool>,
    exact_rerank_count: Option<usize>,
    approximate_candidate_count: Option<usize>,
    semantic_memory_fallback_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct AssistantMessageEvidence {
    citations: Vec<citations::Citation>,
    evidence: ChatEvidenceDisclosure,
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
    std::fs::write(trace_dir.join("latest.json"), bytes)?;
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

fn setting_is_enabled(value: Option<String>) -> bool {
    value
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            normalized == "1" || normalized == "true" || normalized == "yes"
        })
        .unwrap_or(false)
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

    let hybrid_search_ready = crate::state::NATIVE_SEMANTIC_INDEXING_ENABLED
        && !resolved_scope.is_none()
        && state.with_notebook_db(&notebook_id, |db| {
            db.can_run_hybrid_search(resolved_scope.source_ids())
        })?;

    // 3. Only initialize semantic search infrastructure when the selected
    // sources are fully indexed. Otherwise we go straight to the DB/raw
    // fallback path and avoid loading native embedder/index code unnecessarily.
    if hybrid_search_ready {
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
            "Skipping semantic search warmup because selected sources are not fully indexed"
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
    ) = {
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
                app_db.get_setting("semantic_memory_embedding_url")?,
                app_db.get_setting("semantic_memory_embedding_model")?,
                app_db.get_setting("semantic_memory_embedding_timeout_secs")?,
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
    let retrieval_receipt_id = uuid::Uuid::new_v4().to_string();
    #[cfg(feature = "semantic-memory-backend")]
    let mut semantic_memory_receipt: Option<serde_json::Value> = None;
    #[cfg(not(feature = "semantic-memory-backend"))]
    let semantic_memory_receipt: Option<serde_json::Value> = None;
    if !invalid_source_ids.is_empty() {
        retrieval_degradation_markers.push("source-scope-partial-invalid".to_string());
    }

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
                    let reason = format!(
                        "semantic-memory preview timed out after {} ms",
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
                        "semantic-memory preview timed out after {} ms",
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
                        retrieval_fallback_reason = Some(reason);
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
                        })
                        .collect::<Vec<_>>();
                    if context.is_empty() {
                        retrieval_fallback_reason.get_or_insert_with(|| {
                            "semantic-memory preview returned no mapped candidates".to_string()
                        });
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
                            Some(context)
                        }
                    } else {
                        Some(context)
                    }
                }
                Ok(Err(err)) if semantic_fallback_allowed => {
                    let reason = format!("semantic-memory preview failed: {err}");
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
                    let error = err.to_string();
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
        #[cfg(not(feature = "semantic-memory-backend"))]
        {
            if semantic_fallback_allowed {
                let reason =
                    "semantic-memory preview selected but semantic-memory-backend feature is not enabled"
                        .to_string();
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
                        "semantic-memory preview selected but semantic-memory-backend feature is not enabled"
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
        let local_hybrid_results = if force_gloss_local_retrieval {
            None
        } else {
            state.try_hybrid_search(&notebook_id, &query, &resolved_scope, top_k)?
        };
        match local_hybrid_results {
            Some(results) if !results.is_empty() => {
                retrieval_mode = "native-hybrid".to_string();
                retrieval_backend_used = "native-hybrid".to_string();
                // Resolve source titles for each unique source_id
                let unique_source_ids: Vec<String> = results
                    .iter()
                    .map(|r| r.chunk.source_id.clone())
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

                tracing::info!(
                    results = results.len(),
                    top_k,
                    "Hybrid search returned results"
                );

                results
                    .iter()
                    .map(|r| ContextPassage {
                        source_id: r.chunk.source_id.clone(),
                        chunk_id: Some(r.chunk.id.clone()),
                        title: title_map
                            .get(&r.chunk.source_id)
                            .cloned()
                            .unwrap_or_else(|| r.chunk.source_id.clone()),
                        content: r.chunk.content.clone(),
                    })
                    .collect()
            }
            other => {
                // Fallback: first try chunks from DB, then raw content_text
                let reason = match &other {
                    None => "embedder/index not available",
                    Some(_) => "search returned empty results",
                };
                if retrieval_fallback_reason.is_none() {
                    retrieval_fallback_reason = Some(format!("local retrieval fallback: {reason}"));
                }
                if !retrieval_degradation_markers
                    .iter()
                    .any(|marker| marker == "local-retrieval-fallback")
                {
                    retrieval_degradation_markers.push("local-retrieval-fallback".to_string());
                }
                tracing::info!(reason, "Hybrid search unavailable, using fallback context");

                let local_response = state.with_notebook_db(&notebook_id, |db| {
                    let backend =
                        GlossLocalMemoryBackend::new(notebook_id.clone(), db, &all_sources);
                    backend.search(MemorySearchRequest {
                        notebook_id: notebook_id.clone(),
                        source_scope: source_scope.clone(),
                        query: query.clone(),
                        limit: top_k,
                        trace_id: Some(retrieval_receipt_id.clone()),
                        allow_fallback: true,
                    })
                })?;

                if let Some(reason) = local_response.fallback_reason.clone() {
                    retrieval_fallback_reason.get_or_insert(reason);
                }
                if let Some(mode) = local_response
                    .provenance
                    .get("retrieval_mode")
                    .and_then(|value| value.as_str())
                {
                    retrieval_mode = mode.to_string();
                }
                for marker in &local_response.degradation_markers {
                    if !retrieval_degradation_markers.contains(marker) {
                        retrieval_degradation_markers.push(marker.clone());
                    }
                }

                let ranked_ctx = local_response
                    .candidates
                    .into_iter()
                    .map(|candidate| ContextPassage {
                        source_id: candidate.source_id,
                        chunk_id: Some(candidate.chunk_id),
                        title: candidate
                            .source_title
                            .unwrap_or_else(|| "Untitled source".to_string()),
                        content: candidate.content,
                    })
                    .collect::<Vec<_>>();

                if !ranked_ctx.is_empty() {
                    tracing::info!(
                        chunks = ranked_ctx.len(),
                        "Fallback: using gloss-local FTS5/BM25 retrieval"
                    );
                    ranked_ctx
                } else {
                    retrieval_mode = "raw-content-text-fallback".to_string();
                    retrieval_backend_used = "raw-content-text-fallback".to_string();
                    retrieval_fallback_reason.get_or_insert_with(|| {
                        "gloss-local ranked retrieval returned no context; raw content_text fallback"
                            .to_string()
                    });
                    if !retrieval_degradation_markers
                        .iter()
                        .any(|marker| marker == "raw-content-text-fallback")
                    {
                        retrieval_degradation_markers.push("raw-content-text-fallback".to_string());
                    }

                    // Last resort: raw content_text for paste sources or sources without chunks.
                    tracing::info!("Fallback: using raw content_text");
                    state.with_notebook_db(&notebook_id, |db| {
                        let mut ctx = Vec::new();
                        let mut total_chars = 0usize;
                        let mut seen_hashes = HashSet::new();
                        for sid in resolved_scope.source_ids() {
                            if total_chars >= MAX_TOTAL_CONTEXT_CHARS {
                                break;
                            }
                            if let Ok(source) = db.get_source(sid) {
                                if let Some(ref hash) = source.file_hash {
                                    if !seen_hashes.insert(hash.clone()) {
                                        continue;
                                    }
                                }
                                if let Some(ref text) = source.content_text {
                                    if !text.is_empty() {
                                        let remaining =
                                            MAX_TOTAL_CONTEXT_CHARS.saturating_sub(total_chars);
                                        let limit = remaining.min(MAX_SOURCE_CHARS).min(text.len());
                                        let truncated = if limit < text.len() {
                                            let mut safe = limit.min(text.len());
                                            while safe > 0 && !text.is_char_boundary(safe) {
                                                safe -= 1;
                                            }
                                            let slice = &text[..safe];
                                            let end = slice.rfind(' ').unwrap_or(safe);
                                            format!(
                                                "{}...\n[truncated, {} chars total]",
                                                &text[..end],
                                                text.len()
                                            )
                                        } else {
                                            text.clone()
                                        };
                                        total_chars += truncated.len();
                                        ctx.push(ContextPassage {
                                            source_id: source.id.clone(),
                                            chunk_id: None,
                                            title: source.title.clone(),
                                            content: truncated,
                                        });
                                    }
                                }
                            }
                        }
                        Ok(ctx)
                    })?
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
        .map(str::to_string);

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
        omitted_candidate_count: 0,
        source_scope_preserved: true,
        index_status: if resolved_scope.is_none() {
            "scope-none".to_string()
        } else if hybrid_search_ready {
            "indexed".to_string()
        } else {
            "fallback".to_string()
        },
        link_status: if memory_backend == MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW {
            "semantic-memory-links-checked".to_string()
        } else {
            "gloss-local".to_string()
        },
        receipt_id: retrieval_receipt_id,
        semantic_memory_receipt_id,
        candidate_backend,
        turbo_quant_generation_id,
        vector_artifact_manifest_digest,
        exact_rerank,
        exact_rerank_count,
        approximate_candidate_count,
        semantic_memory_fallback_reason,
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
                let extracted =
                    citations::extract_citations_from_context(full_response, &source_context);
                let citation_ref_count = citations::count_unique_citation_refs(full_response);
                let mut evidence = evidence_for_message.clone();
                evidence.citation_valid_count = extracted.len();
                evidence.citation_invalid_count =
                    citation_ref_count.saturating_sub(extracted.len());
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

    // Build system prompt with source manifest + selected source content.
    let system_prompt = ContextAssembler::build_system_prompt(
        custom_goal,
        style,
        resolved_scope.kind(),
        resolved_scope.manifest_sources(),
        source_context,
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

    // User message is just the query — source context is in the system prompt
    chat_messages.push(ChatMessage {
        role: "user".to_string(),
        content: query.to_string(),
        images: None,
    });

    let max_tokens = 2048;
    let num_ctx = compute_dynamic_num_ctx(
        &system_prompt,
        &chat_messages,
        model_context_window,
        max_tokens,
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
                    record_chat_attempt_trace(
                        attempt_trace,
                        trace_data_dir,
                        "provider_start_error",
                        Some(started.elapsed()),
                        Some("Provider failed before returning a stream"),
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
                record_chat_attempt_trace(
                    attempt_trace,
                    trace_data_dir,
                    "provider_stream_error",
                    Some(started.elapsed()),
                    Some("Provider stream yielded an error"),
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
    let path = state
        .data_dir
        .join("chat-attempt-traces")
        .join("latest.json");
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)?;
    Ok(Some(serde_json::from_str(&text)?))
}
