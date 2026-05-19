use crate::db::notebook_db::Note;
use crate::error::GlossError;
use crate::state::AppState;
use tauri::State;

#[tauri::command]
pub async fn list_notes(
    notebook_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<Note>, GlossError> {
    state.with_notebook_db(&notebook_id, |db| db.list_notes())
}

#[tauri::command]
pub async fn create_note(
    notebook_id: String,
    title: String,
    content: String,
    state: State<'_, AppState>,
) -> Result<String, GlossError> {
    let id = uuid::Uuid::new_v4().to_string();
    let note = Note {
        id: id.clone(),
        title: Some(title),
        content,
        note_type: "manual".to_string(),
        citations: None,
        pinned: false,
        source_id: None,
        created_at: String::new(),
        updated_at: String::new(),
    };
    state.with_notebook_db(&notebook_id, |db| db.create_note(&note))?;
    Ok(id)
}

#[tauri::command]
pub async fn save_response_as_note(
    notebook_id: String,
    message_id: String,
    state: State<'_, AppState>,
) -> Result<String, GlossError> {
    let msg = state.with_notebook_db(&notebook_id, |db| db.get_message(&message_id))?;

    let id = uuid::Uuid::new_v4().to_string();
    let title = msg.content.chars().take(60).collect::<String>();
    let backlink_section = msg
        .citations
        .as_deref()
        .and_then(citation_backlinks)
        .unwrap_or_default();
    let note = Note {
        id: id.clone(),
        title: Some(title),
        content: format!("{}{}", msg.content, backlink_section),
        note_type: "saved_response".to_string(),
        citations: msg.citations,
        pinned: true,
        source_id: None,
        created_at: String::new(),
        updated_at: String::new(),
    };
    state.with_notebook_db(&notebook_id, |db| db.create_note(&note))?;
    Ok(id)
}

fn citation_backlinks(citations_json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(citations_json).ok()?;
    let citations = if let Some(items) = value.as_array() {
        items
    } else {
        value.get("citations")?.as_array()?
    };
    if citations.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    for (idx, citation) in citations.iter().enumerate() {
        let source_id = citation.get("source_id")?.as_str()?;
        let chunk_id = citation.get("chunk_id")?.as_str()?;
        let title = citation
            .get("source_title")
            .and_then(|v| v.as_str())
            .unwrap_or(source_id);
        lines.push(format!(
            "{}. {} (source: {}, chunk: {})",
            idx + 1,
            title,
            source_id,
            chunk_id
        ));
    }

    if lines.is_empty() {
        None
    } else {
        Some(format!("\n\nSources\n{}", lines.join("\n")))
    }
}

#[tauri::command]
pub async fn update_note(
    notebook_id: String,
    note_id: String,
    title: Option<String>,
    content: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), GlossError> {
    state.with_notebook_db(&notebook_id, |db| {
        db.update_note(&note_id, title.as_deref(), content.as_deref())
    })
}

#[tauri::command]
pub async fn toggle_pin(
    notebook_id: String,
    note_id: String,
    state: State<'_, AppState>,
) -> Result<(), GlossError> {
    state.with_notebook_db(&notebook_id, |db| db.toggle_pin(&note_id))
}

#[tauri::command]
pub async fn delete_note(
    notebook_id: String,
    note_id: String,
    state: State<'_, AppState>,
) -> Result<(), GlossError> {
    state.with_notebook_db(&notebook_id, |db| db.delete_note(&note_id))
}
