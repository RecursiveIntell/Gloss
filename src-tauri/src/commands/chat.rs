use crate::db::notebook_db::{Conversation, Message, Source};
use crate::error::GlossError;
use crate::retrieval::context::ContextAssembler;
use crate::retrieval::hybrid_search;
use crate::state::AppState;
use futures::StreamExt;
use std::collections::{HashMap, HashSet};
use tauri::{Emitter, State};

/// Maximum characters of source content to inject per source (fallback path).
const MAX_SOURCE_CHARS: usize = 8_000;
/// Maximum total characters of all source context combined (fallback path).
const MAX_TOTAL_CONTEXT_CHARS: usize = 32_000;

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
    app_handle: tauri::AppHandle,
) -> Result<String, GlossError> {
    let message_id = uuid::Uuid::new_v4().to_string();

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

    // Get provider
    let provider = {
        let registry = state
            .model_registry
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        registry.get_provider_for_model(&model).is_some()
    };

    if !provider {
        return Err(GlossError::Config(format!(
            "No provider found for model {}",
            model
        )));
    }

    // --- RAG context assembly ---
    // 1. Load all sources upfront for manifest + ID validation
    let all_sources: Vec<Source> =
        state.with_notebook_db(&notebook_id, |db| db.list_sources())?;
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

    // 3. Ensure embedder and index are initialized (no-op if already done)
    if let Err(e) = state.ensure_embedder(Some(&app_handle)) {
        tracing::warn!("Embedder init failed (will fall back to raw context): {}", e);
    }
    if let Err(e) = state.ensure_hnsw_index(&notebook_id) {
        tracing::warn!("HNSW index init failed (will fall back to raw context): {}", e);
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
    let source_context: Vec<(String, String)> = match state.try_hybrid_search(
        &notebook_id,
        &query,
        &effective_source_ids,
        top_k,
    )? {
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
            let chunk_ctx: Vec<(String, String)> = state.with_notebook_db(&notebook_id, |db| {
                let mut ctx = Vec::new();
                let mut total_chars = 0usize;

                for sid in &effective_source_ids {
                    if total_chars >= MAX_TOTAL_CONTEXT_CHARS {
                        break;
                    }
                    if let Ok(chunks) = db.get_chunks_for_source(sid) {
                        let title = title_map.get(sid).cloned().unwrap_or_else(|| sid.clone());
                        for chunk in &chunks {
                            if total_chars >= MAX_TOTAL_CONTEXT_CHARS {
                                break;
                            }
                            let limit = MAX_SOURCE_CHARS.min(
                                MAX_TOTAL_CONTEXT_CHARS.saturating_sub(total_chars),
                            );
                            let text = if chunk.content.len() > limit {
                                let slice = &chunk.content[..limit];
                                let end = slice.rfind(' ').unwrap_or(limit);
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
                                    let limit =
                                        remaining.min(MAX_SOURCE_CHARS).min(text.len());
                                    let truncated = if limit < text.len() {
                                        let slice = &text[..limit];
                                        let end = slice.rfind(' ').unwrap_or(limit);
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

    // Bump chat grace window: summaries must not start for 15s after user message
    state.bump_chat_grace();
    state.bump_user_activity();

    // For streaming, we spawn an async task
    let msg_id = message_id.clone();
    let conv_id = conversation_id.clone();
    let nb_id = notebook_id.clone();
    let handle = app_handle.clone();

    // Get the Ollama base URL for direct HTTP calls
    let base_url = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        app_db
            .get_setting("ollama_url")?
            .unwrap_or_else(|| "http://localhost:11434".to_string())
    };

    tokio::spawn(async move {
        use tauri::Manager;
        let app_state: tauri::State<'_, AppState> = handle.state();

        // Acquire single-flight LLM gate (waits for any in-flight summary to finish)
        let permit = match tokio::time::timeout(
            std::time::Duration::from_secs(120),
            app_state.llm_gate.acquire(),
        ).await {
            Ok(Ok(p)) => p,
            Ok(Err(_)) => {
                tracing::error!(message_id = %msg_id, "LLM gate closed during chat");
                let _ = handle.emit(
                    "chat:error",
                    serde_json::json!({
                        "conversation_id": conv_id,
                        "message_id": msg_id,
                        "error": "LLM gate closed — app may be shutting down",
                    }),
                );
                return;
            }
            Err(_timeout) => {
                tracing::warn!(message_id = %msg_id, "Timed out waiting 120s for LLM gate");
                let _ = handle.emit(
                    "chat:error",
                    serde_json::json!({
                        "conversation_id": conv_id,
                        "message_id": msg_id,
                        "error": "Timed out waiting for GPU. A background task may be stuck — try pausing summaries or restarting the app.",
                    }),
                );
                let _ = handle.emit(
                    "chat:token",
                    serde_json::json!({
                        "conversation_id": conv_id,
                        "message_id": msg_id,
                        "token": "",
                        "done": true,
                    }),
                );
                return;
            }
        };

        let result = stream_chat_response(
            &handle,
            &base_url,
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
                // Persist assistant message to DB
                let assistant_msg = Message {
                    id: msg_id.clone(),
                    conversation_id: conv_id.clone(),
                    role: "assistant".to_string(),
                    content: full_response.clone(),
                    citations: None,
                    model_used: Some(model.clone()),
                    tokens_prompt: None,
                    tokens_response: None,
                    created_at: String::new(),
                };
                if let Err(e) = app_state.with_notebook_db(&nb_id, |db| db.insert_message(&assistant_msg)) {
                    tracing::error!(message_id = %msg_id, "Failed to persist assistant message: {}", e);
                }
            }
            Err(e) => {
                tracing::error!(message_id = %msg_id, "Chat streaming failed: {}", e);
                // Emit structured error event — never append error text into assistant content
                let _ = handle.emit(
                    "chat:error",
                    serde_json::json!({
                        "conversation_id": conv_id,
                        "message_id": msg_id,
                        "error": e.to_string(),
                    }),
                );
                // Send a done signal so frontend stops streaming state
                let _ = handle.emit(
                    "chat:token",
                    serde_json::json!({
                        "conversation_id": conv_id,
                        "message_id": msg_id,
                        "token": "",
                        "done": true,
                    }),
                );
            }
        }

        // Release LLM gate
        drop(permit);
    });

    Ok(message_id)
}

#[allow(clippy::too_many_arguments)]
async fn stream_chat_response(
    app_handle: &tauri::AppHandle,
    base_url: &str,
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
    // Build system prompt with source manifest + selected source content.
    let system_prompt =
        ContextAssembler::build_system_prompt(custom_goal, style, all_sources, source_context);

    tracing::info!(
        system_prompt_len = system_prompt.len(),
        "System prompt built for LLM"
    );

    // Build messages: system first, then history, then the clean user query
    let mut messages: Vec<serde_json::Value> = Vec::new();

    messages.push(serde_json::json!({
        "role": "system",
        "content": system_prompt,
    }));

    let history_msgs = ContextAssembler::format_history(history, 10);
    for (role, content) in &history_msgs {
        messages.push(serde_json::json!({
            "role": role,
            "content": content,
        }));
    }

    // User message is just the query — source context is in the system prompt
    messages.push(serde_json::json!({
        "role": "user",
        "content": query,
    }));

    // Make streaming request to Ollama with timeout
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| GlossError::Provider {
            provider: "ollama".into(),
            source: e.into(),
        })?;

    let url = format!("{}/api/chat", base_url);
    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "options": {
            "temperature": 0.7,
            "num_predict": 2048,
            "num_ctx": 16384,
        }
    });

    let resp = client
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
        let text = resp.text().await.unwrap_or_default();
        // Detect CUDA errors and provide actionable message
        let msg = if text.contains("CUDA") || text.contains("illegal memory access") {
            format!(
                "GPU error (CUDA illegal memory access). Try: restart Ollama, or switch to a CPU-only model. Raw: HTTP {}: {}",
                status, text
            )
        } else {
            format!("HTTP {}: {}", status, text)
        };
        return Err(GlossError::Provider {
            provider: "ollama".into(),
            source: anyhow::anyhow!("{}", msg),
        });
    }

    // Stream tokens
    let mut decoder = llm_pipeline::StreamingDecoder::new();
    let mut full_response = String::new();
    let mut byte_stream = resp.bytes_stream();
    let mut sent_done = false;

    while let Some(chunk_result) = byte_stream.next().await {
        let bytes = chunk_result.map_err(|e| GlossError::Provider {
            provider: "ollama".into(),
            source: e.into(),
        })?;

        let values = decoder.decode(&bytes);
        for val in values {
            // Check for Ollama error response
            if let Some(error_msg) = val.get("error").and_then(|e| e.as_str()) {
                let msg = if error_msg.contains("CUDA") || error_msg.contains("illegal memory access") {
                    format!(
                        "GPU error (CUDA illegal memory access). Try: restart Ollama, or switch to a CPU-only model. Raw: {}",
                        error_msg
                    )
                } else {
                    error_msg.to_string()
                };
                return Err(GlossError::Provider {
                    provider: "ollama".into(),
                    source: anyhow::anyhow!("{}", msg),
                });
            }

            let done = val
                .get("done")
                .and_then(|d| d.as_bool())
                .unwrap_or(false);
            let token = val
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();

            full_response.push_str(&token);

            if done {
                sent_done = true;
            }

            let _ = app_handle.emit(
                "chat:token",
                serde_json::json!({
                    "conversation_id": conversation_id,
                    "message_id": message_id,
                    "token": token,
                    "done": done,
                }),
            );
        }
    }

    // Flush remaining
    if let Some(val) = decoder.flush() {
        let token = val
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        full_response.push_str(&token);
        sent_done = true;

        let _ = app_handle.emit(
            "chat:token",
            serde_json::json!({
                "conversation_id": conversation_id,
                "message_id": message_id,
                "token": token,
                "done": true,
            }),
        );
    }

    // Guarantee a done signal — prevents frontend from hanging forever
    if !sent_done {
        let _ = app_handle.emit(
            "chat:token",
            serde_json::json!({
                "conversation_id": conversation_id,
                "message_id": message_id,
                "token": "",
                "done": true,
            }),
        );
    }

    tracing::debug!(message_id, len = full_response.len(), "Chat response complete");

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
