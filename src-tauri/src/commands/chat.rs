use crate::db::notebook_db::{Conversation, Message};
use crate::error::GlossError;
use crate::retrieval::context::ContextAssembler;
use crate::state::AppState;
use futures::StreamExt;
use tauri::{Emitter, State};

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

#[tauri::command]
pub async fn send_message(
    notebook_id: String,
    conversation_id: String,
    query: String,
    _selected_source_ids: Vec<String>,
    model: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<String, GlossError> {
    let message_id = uuid::Uuid::new_v4().to_string();

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
        let registry = state.model_registry.lock().map_err(|e| GlossError::Other(e.to_string()))?;
        registry.get_provider_for_model(&model).is_some()
    };

    if !provider {
        return Err(GlossError::Config(format!("No provider found for model {}", model)));
    }

    // For streaming, we spawn an async task
    let msg_id = message_id.clone();
    let conv_id = conversation_id.clone();
    let nb_id = notebook_id.clone();
    let handle = app_handle.clone();

    // Get history and notebook context
    let (history, custom_goal, style) = state.with_notebook_db(&notebook_id, |db| {
        let history = db.load_messages(&conversation_id)?;
        let goal = db.get_config("custom_goal")?;
        let style = db.get_config("default_style")?.unwrap_or_else(|| "default".to_string());
        Ok((history, goal, style))
    })?;

    // Get the Ollama base URL for direct HTTP calls
    let base_url = {
        let app_db = state.app_db.lock().map_err(|e| GlossError::Other(e.to_string()))?;
        app_db.get_setting("ollama_url")?.unwrap_or_else(|| "http://localhost:11434".to_string())
    };

    tokio::spawn(async move {
        let result = stream_chat_response(
            &handle,
            &base_url,
            &nb_id,
            &conv_id,
            &msg_id,
            &query,
            &model,
            &history,
            custom_goal.as_deref(),
            &style,
        )
        .await;

        if let Err(e) = result {
            tracing::error!(message_id = %msg_id, "Chat streaming failed: {}", e);
            let _ = handle.emit(
                "chat:token",
                serde_json::json!({
                    "conversation_id": conv_id,
                    "message_id": msg_id,
                    "token": format!("\n\n[Error: {}]", e),
                    "done": true,
                }),
            );
        }
    });

    Ok(message_id)
}

#[allow(clippy::too_many_arguments)]
async fn stream_chat_response(
    app_handle: &tauri::AppHandle,
    base_url: &str,
    _notebook_id: &str,
    conversation_id: &str,
    message_id: &str,
    query: &str,
    model: &str,
    history: &[Message],
    custom_goal: Option<&str>,
    style: &str,
) -> Result<(), GlossError> {
    // Build system prompt
    let system_prompt = ContextAssembler::build_system_prompt(custom_goal, style);

    // Build messages from history
    let history_msgs = ContextAssembler::format_history(history, 10);
    let mut messages: Vec<serde_json::Value> = Vec::new();

    for (role, content) in &history_msgs {
        messages.push(serde_json::json!({
            "role": role,
            "content": content,
        }));
    }
    messages.push(serde_json::json!({
        "role": "user",
        "content": query,
    }));

    // Make streaming request to Ollama
    let client = reqwest::Client::new();
    let url = format!("{}/api/chat", base_url);
    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "system": system_prompt,
        "stream": true,
        "options": {
            "temperature": 0.7,
            "num_predict": 1536,
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
        return Err(GlossError::Provider {
            provider: "ollama".into(),
            source: anyhow::anyhow!("HTTP {}: {}", status, text),
        });
    }

    // Stream tokens
    let mut decoder = llm_pipeline::StreamingDecoder::new();
    let mut full_response = String::new();
    let mut byte_stream = resp.bytes_stream();

    while let Some(chunk_result) = byte_stream.next().await {
        let bytes = chunk_result.map_err(|e| GlossError::Provider {
            provider: "ollama".into(),
            source: e.into(),
        })?;

        let values = decoder.decode(&bytes);
        for val in values {
            let done = val.get("done").and_then(|d| d.as_bool()).unwrap_or(false);
            let token = val
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();

            full_response.push_str(&token);

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

    // Store the complete assistant message
    // Note: in the full implementation, this would also store citations
    // from hybrid_search results. For now, we store the raw response.
    let _assistant_msg = Message {
        id: message_id.to_string(),
        conversation_id: conversation_id.to_string(),
        role: "assistant".to_string(),
        content: full_response,
        citations: None,
        model_used: Some(model.to_string()),
        tokens_prompt: None,
        tokens_response: None,
        created_at: String::new(),
    };

    // We need to store the message, but we don't have direct state access in the spawned task.
    // This is handled by the frontend updating on "done" signal.
    // TODO: Store message via AppHandle state access
    tracing::debug!(message_id, "Chat response complete");

    Ok(())
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
