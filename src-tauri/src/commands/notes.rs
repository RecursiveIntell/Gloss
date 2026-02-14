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
    let note = Note {
        id: id.clone(),
        title: Some(title),
        content: msg.content,
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
