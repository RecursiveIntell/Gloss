use crate::db::notebook_db::{Conversation, Message, Source};
use crate::error::GlossError;
use crate::jobs;
use crate::providers::{self, ChatMessage, ChatRequest, ChatToken, LlmProvider};
use crate::retrieval::citations;
use crate::retrieval::context::ContextAssembler;
use crate::retrieval::hybrid_search;
use crate::state::AppState;
use futures::StreamExt;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{Emitter, State};
use tauri_queue::QueueManager;
use tokio::sync::TryAcquireError;

/// Maximum characters of source content to inject per source (fallback path).
const MAX_SOURCE_CHARS: usize = 8_000;
/// Maximum total characters of all source context combined (fallback path).
const MAX_TOTAL_CONTEXT_CHARS: usize = 32_000;
const CHAT_CANCELLED_NOTEBOOK_SWITCH: &str = "__chat_cancelled_notebook_switch__";

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

fn emit_chat_error_and_done(
    handle: &tauri::AppHandle,
    notebook_id: &str,
    conversation_id: &str,
    message_id: &str,
    error: &str,
) {
    emit_chat_error(handle, notebook_id, conversation_id, message_id, error);
    emit_chat_done(handle, notebook_id, conversation_id, message_id);
}

async fn acquire_gate_with_epoch<'a>(
    state: &'a AppState,
    notebook_id: &str,
    epoch: u64,
    gate: &'a tokio::sync::Semaphore,
    timeout: Duration,
    gate_name: &str,
) -> Result<Option<tokio::sync::SemaphorePermit<'a>>, GlossError> {
    let started = Instant::now();

    loop {
        if !state.is_active_notebook_epoch(notebook_id, epoch) {
            return Ok(None);
        }

        match gate.try_acquire() {
            Ok(permit) => return Ok(Some(permit)),
            Err(TryAcquireError::NoPermits) => {
                if started.elapsed() >= timeout {
                    return Err(GlossError::Other(format!(
                        "Timed out waiting for {gate_name}."
                    )));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(TryAcquireError::Closed) => {
                return Err(GlossError::Other(format!("{gate_name} closed")));
            }
        }
    }
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
    selected_source_ids: Vec<String>,
    model: String,
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
    app_handle: tauri::AppHandle,
) -> Result<String, GlossError> {
    let message_id = uuid::Uuid::new_v4().to_string();
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

    // Get provider config (short lock, no await)
    let provider_config = {
        let registry = state
            .model_registry
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        registry.get_provider_config_for_model(&model, &app_db)?
    };

    // --- RAG context assembly ---
    // 1. Load all sources upfront for manifest + ID validation
    let all_sources: Vec<Source> = state.with_notebook_db(&notebook_id, |db| db.list_sources())?;
    let all_source_ids: HashSet<String> = all_sources.iter().map(|s| s.id.clone()).collect();

    // 2. Compute effective_source_ids with validation
    let effective_source_ids: Vec<String> = if selected_source_ids.is_empty() {
        tracing::debug!("No sources explicitly selected — using all sources");
        all_source_ids.iter().cloned().collect()
    } else {
        // Validate selected IDs against this notebook's actual sources
        let valid: Vec<String> = selected_source_ids
            .iter()
            .filter(|id| all_source_ids.contains(id.as_str()))
            .cloned()
            .collect();
        if valid.is_empty() && !all_source_ids.is_empty() {
            tracing::warn!(
                sent_ids = selected_source_ids.len(),
                notebook_sources = all_source_ids.len(),
                "All selected source IDs are stale/invalid — using all notebook sources"
            );
            all_source_ids.iter().cloned().collect()
        } else if valid.len() < selected_source_ids.len() {
            tracing::debug!(
                valid = valid.len(),
                sent = selected_source_ids.len(),
                "Some selected source IDs were invalid, using {} valid ones",
                valid.len()
            );
            valid
        } else {
            valid
        }
    };

    let hybrid_search_ready = crate::state::NATIVE_SEMANTIC_INDEXING_ENABLED
        && state.with_notebook_db(&notebook_id, |db| {
            db.can_run_hybrid_search(&effective_source_ids)
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
            selected_sources = effective_source_ids.len(),
            "Skipping semantic search warmup because selected sources are not fully indexed"
        );
    }

    let source_count = all_sources.len();
    let top_k = hybrid_search::compute_top_k(source_count);

    tracing::info!(
        selected_source_ids = selected_source_ids.len(),
        effective_source_ids = effective_source_ids.len(),
        source_count,
        top_k,
        "Starting RAG context assembly"
    );

    // 4. Hybrid search with multi-tier fallback
    let source_context: Vec<(String, String)> =
        match state.try_hybrid_search(&notebook_id, &query, &effective_source_ids, top_k)? {
            Some(results) if !results.is_empty() => {
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
                    .map(|r| {
                        let title = title_map
                            .get(&r.chunk.source_id)
                            .cloned()
                            .unwrap_or_else(|| r.chunk.source_id.clone());
                        (title, r.chunk.content.clone())
                    })
                    .collect()
            }
            other => {
                // Fallback: first try chunks from DB, then raw content_text
                let reason = match &other {
                    None => "embedder/index not available",
                    Some(_) => "search returned empty results",
                };
                tracing::info!(reason, "Hybrid search unavailable, using fallback context");

                // Build title map from already-loaded all_sources
                let title_map: HashMap<String, String> = all_sources
                    .iter()
                    .map(|s| (s.id.clone(), s.title.clone()))
                    .collect();

                // Strategy 1: Load chunks directly from DB
                let chunk_ctx: Vec<(String, String)> =
                    state.with_notebook_db(&notebook_id, |db| {
                        let mut ctx = Vec::new();
                        let mut total_chars = 0usize;

                        for sid in &effective_source_ids {
                            if total_chars >= MAX_TOTAL_CONTEXT_CHARS {
                                break;
                            }
                            if let Ok(chunks) = db.get_chunks_for_source(sid) {
                                let title =
                                    title_map.get(sid).cloned().unwrap_or_else(|| sid.clone());
                                for chunk in &chunks {
                                    if total_chars >= MAX_TOTAL_CONTEXT_CHARS {
                                        break;
                                    }
                                    let limit = MAX_SOURCE_CHARS
                                        .min(MAX_TOTAL_CONTEXT_CHARS.saturating_sub(total_chars));
                                    let text = if chunk.content.len() > limit {
                                        let mut safe = limit.min(chunk.content.len());
                                        while safe > 0 && !chunk.content.is_char_boundary(safe) {
                                            safe -= 1;
                                        }
                                        let slice = &chunk.content[..safe];
                                        let end = slice.rfind(' ').unwrap_or(safe);
                                        format!("{}...", &chunk.content[..end])
                                    } else {
                                        chunk.content.clone()
                                    };
                                    total_chars += text.len();
                                    ctx.push((title.clone(), text));
                                }
                            }
                        }
                        Ok(ctx)
                    })?;

                if !chunk_ctx.is_empty() {
                    tracing::info!(chunks = chunk_ctx.len(), "Fallback: using DB chunks");
                    chunk_ctx
                } else {
                    // Strategy 2: raw content_text (paste sources, or sources without chunks)
                    tracing::info!("Fallback: using raw content_text");
                    state.with_notebook_db(&notebook_id, |db| {
                        let mut ctx = Vec::new();
                        let mut total_chars = 0usize;
                        let mut seen_hashes = HashSet::new();
                        for sid in &effective_source_ids {
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
                                        ctx.push((source.title.clone(), truncated));
                                    }
                                }
                            }
                        }
                        Ok(ctx)
                    })?
                }
            }
        };

    tracing::info!(
        context_passages = source_context.len(),
        manifest_sources = all_sources.len(),
        context_chars = source_context.iter().map(|(_, c)| c.len()).sum::<usize>(),
        "RAG context assembled for chat"
    );

    if !state.is_active_notebook_epoch(&notebook_id, request_epoch) {
        tracing::info!(
            notebook_id = %notebook_id,
            epoch = request_epoch,
            "Notebook changed during chat preparation; skipping stream start"
        );
        emit_chat_done(&app_handle, &notebook_id, &conversation_id, &message_id);
        return Ok(message_id);
    }

    // For streaming, we spawn an async task
    let msg_id = message_id.clone();
    let conv_id = conversation_id.clone();
    let nb_id = notebook_id.clone();
    let epoch = request_epoch;
    let handle = app_handle.clone();

    // Construct the provider outside any lock (provider_config was extracted above)
    let provider = providers::build_provider(&provider_config);

    tokio::spawn(async move {
        use tauri::Manager;
        let app_state: tauri::State<'_, AppState> = handle.state();

        // Acquire single-flight LLM gate (waits for any in-flight summary to finish).
        // We poll with notebook/epoch checks so a notebook switch cancels cleanly
        // instead of waiting for the full timeout.
        let permit = match acquire_gate_with_epoch(
            &app_state,
            &nb_id,
            epoch,
            &app_state.llm_gate,
            Duration::from_secs(120),
            "LLM gate",
        )
        .await
        {
            Ok(Some(permit)) => permit,
            Ok(None) => {
                emit_chat_done(&handle, &nb_id, &conv_id, &msg_id);
                return;
            }
            Err(e) => {
                tracing::error!(message_id = %msg_id, error = %e, "LLM gate acquisition failed");
                emit_chat_error(&handle, &nb_id, &conv_id, &msg_id, &e.to_string());
                emit_chat_done(&handle, &nb_id, &conv_id, &msg_id);
                return;
            }
        };

        let gpu_permit = match acquire_gate_with_epoch(
            &app_state,
            &nb_id,
            epoch,
            &app_state.gpu_gate,
            Duration::from_secs(120),
            "GPU gate",
        )
        .await
        {
            Ok(Some(permit)) => permit,
            Ok(None) => {
                drop(permit);
                emit_chat_done(&handle, &nb_id, &conv_id, &msg_id);
                return;
            }
            Err(e) => {
                drop(permit);
                emit_chat_error(&handle, &nb_id, &conv_id, &msg_id, &e.to_string());
                emit_chat_done(&handle, &nb_id, &conv_id, &msg_id);
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
            &all_sources,
            &source_context,
        )
        .await;

        match &result {
            Ok(full_response) => {
                if !app_state.is_active_notebook_epoch(&nb_id, epoch) {
                    emit_chat_done(&handle, &nb_id, &conv_id, &msg_id);
                    drop(gpu_permit);
                    drop(permit);
                    return;
                }

                // Extract citations from the response
                let extracted =
                    citations::extract_citations_from_context(full_response, &source_context);
                let citations_json = if extracted.is_empty() {
                    None
                } else {
                    serde_json::to_string(&extracted).ok()
                };

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
                }
            }
            Err(e) => {
                if e.to_string() != CHAT_CANCELLED_NOTEBOOK_SWITCH {
                    tracing::error!(message_id = %msg_id, "Chat streaming failed: {}", e);
                    emit_chat_error(&handle, &nb_id, &conv_id, &msg_id, &e.to_string());
                }
                emit_chat_done(&handle, &nb_id, &conv_id, &msg_id);
            }
        }

        // Release gates
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
    all_sources: &[Source],
    source_context: &[(String, String)],
) -> Result<String, GlossError> {
    use tauri::Manager;

    // Build system prompt with source manifest + selected source content.
    let system_prompt =
        ContextAssembler::build_system_prompt(custom_goal, style, all_sources, source_context);

    tracing::info!(
        system_prompt_len = system_prompt.len(),
        provider = provider.provider_type().as_str(),
        "System prompt built for LLM"
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

    // Build the provider-agnostic chat request
    let request = ChatRequest {
        model: model.to_string(),
        system_prompt: Some(system_prompt),
        messages: chat_messages,
        max_tokens: 2048,
        temperature: 0.7,
        stream: true,
        num_ctx: Some(16384), // CRITICAL: prevents Ollama from truncating system prompt
    };

    let state: tauri::State<'_, AppState> = app_handle.state();

    if !state.is_active_notebook_epoch(notebook_id, epoch) {
        return Err(GlossError::Other(CHAT_CANCELLED_NOTEBOOK_SWITCH.into()));
    }

    // Call the provider, but keep checking notebook epoch while waiting for the
    // first response so a switch can cancel the HTTP request promptly.
    let chat_future = provider.chat(request);
    tokio::pin!(chat_future);
    let mut token_stream = loop {
        if !state.is_active_notebook_epoch(notebook_id, epoch) {
            return Err(GlossError::Other(CHAT_CANCELLED_NOTEBOOK_SWITCH.into()));
        }

        tokio::select! {
            result = &mut chat_future => break result?,
            _ = tokio::time::sleep(Duration::from_millis(250)) => {}
        }
    };

    let mut full_response = String::new();
    let mut sent_done = false;

    loop {
        if !state.is_active_notebook_epoch(notebook_id, epoch) {
            return Err(GlossError::Other(CHAT_CANCELLED_NOTEBOOK_SWITCH.into()));
        }

        let next = match tokio::time::timeout(Duration::from_millis(250), token_stream.next()).await
        {
            Ok(next) => next,
            Err(_) => continue,
        };
        let Some(result) = next else {
            break;
        };

        let ChatToken { token, done } = result?;

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
                "done": done,
            }),
        );
    }

    // Guarantee a done signal — prevents frontend from hanging forever
    if !sent_done {
        let _ = app_handle.emit(
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
    // For Phase 1, return cached questions from notebook_config or empty
    let questions = state.with_notebook_db(&notebook_id, |db| {
        match db.get_config("suggested_questions")? {
            Some(json) => {
                let qs: Vec<String> = serde_json::from_str(&json).unwrap_or_default();
                Ok(qs)
            }
            None => Ok(Vec::new()),
        }
    })?;
    Ok(questions)
}
