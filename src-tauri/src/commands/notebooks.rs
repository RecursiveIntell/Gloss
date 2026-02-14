use crate::db::app_db::Notebook;
use crate::error::GlossError;
use crate::state::AppState;
use tauri::State;

#[tauri::command]
pub async fn list_notebooks(state: State<'_, AppState>) -> Result<Vec<Notebook>, GlossError> {
    let app_db = state.app_db.lock().map_err(|e| GlossError::Other(e.to_string()))?;
    app_db.list_notebooks()
}

#[tauri::command]
pub async fn create_notebook(
    name: String,
    state: State<'_, AppState>,
) -> Result<String, GlossError> {
    let id = uuid::Uuid::new_v4().to_string();
    let nb_dir = state.data_dir.join("notebooks").join(&id);

    // Create notebook directories
    std::fs::create_dir_all(nb_dir.join("sources"))?;
    std::fs::create_dir_all(nb_dir.join("embeddings"))?;
    std::fs::create_dir_all(nb_dir.join("audio"))?;
    std::fs::create_dir_all(nb_dir.join("exports"))?;

    let dir_str = nb_dir.to_string_lossy().to_string();

    // Register in app DB
    {
        let app_db = state.app_db.lock().map_err(|e| GlossError::Other(e.to_string()))?;
        app_db.create_notebook(&id, &name, &dir_str)?;
    }

    // Open the notebook DB (creates it with migrations)
    state.get_notebook_db(&id)?;

    tracing::info!(id = %id, name = %name, "Created notebook");
    Ok(id)
}

#[tauri::command]
pub async fn delete_notebook(
    id: String,
    state: State<'_, AppState>,
) -> Result<(), GlossError> {
    // Get directory before deleting from DB
    let dir = {
        let app_db = state.app_db.lock().map_err(|e| GlossError::Other(e.to_string()))?;
        let nb = app_db.get_notebook(&id)?;
        app_db.delete_notebook(&id)?;
        nb.directory
    };

    // Remove from open notebooks
    {
        let mut dbs = state.notebook_dbs.lock().map_err(|e| GlossError::Other(e.to_string()))?;
        dbs.remove(&id);
    }

    // Delete the notebook directory
    let dir_path = std::path::PathBuf::from(&dir);
    if dir_path.exists() {
        std::fs::remove_dir_all(&dir_path)?;
    }

    tracing::info!(id = %id, "Deleted notebook");
    Ok(())
}
