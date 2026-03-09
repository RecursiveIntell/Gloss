use crate::db::app_db::Notebook;
use crate::error::GlossError;
use crate::jobs;
use crate::state::AppState;
use std::sync::Arc;
use tauri::{Manager, State};
use tauri_queue::QueueManager;

#[tauri::command]
pub async fn list_notebooks(state: State<'_, AppState>) -> Result<Vec<Notebook>, GlossError> {
    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
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
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        app_db.create_notebook(&id, &name, &dir_str)?;
    }

    // Create the notebook DB and run its initial migrations once.
    crate::db::notebook_db::NotebookDb::open(&nb_dir.join("notebook.db"))?;

    tracing::info!(id = %id, name = %name, "Created notebook");
    Ok(id)
}

#[tauri::command]
pub async fn delete_notebook(id: String, state: State<'_, AppState>) -> Result<(), GlossError> {
    // If this is the active notebook, clear it and bump epoch so the summary
    // loop stops picking up jobs for it immediately.
    if state.get_active_notebook_id().as_deref() == Some(id.as_str()) {
        state.set_active_notebook(None);
    }

    // Get directory before deleting from DB
    let dir = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let nb = app_db.get_notebook(&id)?;
        app_db.delete_notebook(&id)?;
        nb.directory
    };

    // Remove from open notebooks
    {
        let mut dbs = state
            .notebook_dbs
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        dbs.remove(&id);
    }

    // Remove HNSW index from memory
    {
        let mut indices = state
            .hnsw_indices
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        indices.remove(&id);
    }

    // Delete the notebook directory
    let dir_path = std::path::PathBuf::from(&dir);
    if dir_path.exists() {
        std::fs::remove_dir_all(&dir_path)?;
    }

    tracing::info!(id = %id, "Deleted notebook");
    Ok(())
}

/// Set (or clear) the active notebook for scheduling purposes.
/// The summary worker will idle when no notebook is active.
/// Increments the epoch counter so stale summary jobs are soft-cancelled.
/// Eagerly initializes the embedder and HNSW index so the first chat
/// message doesn't have to wait for model loading.
#[tauri::command]
pub async fn set_active_notebook(
    notebook_id: Option<String>,
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
    app_handle: tauri::AppHandle,
) -> Result<(), GlossError> {
    state.set_active_notebook(notebook_id.clone());
    let active_epoch = state.get_active_epoch();

    let cancelled = jobs::cancel_jobs_not_matching_active_notebook(
        &queue,
        notebook_id.as_deref(),
        active_epoch,
    );
    if cancelled > 0 {
        tracing::info!(
            cancelled,
            "Cancelled stale background jobs after notebook switch"
        );
    }

    // Warm the notebook DB in the background, but avoid eager native
    // embedder/HNSW initialization here. Those paths are only needed when a
    // fully indexed scope is actually queried, and keeping them out of notebook
    // switching reduces native crash surface during imports.
    if let Some(nb_id) = notebook_id {
        let handle = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            let _ = tokio::task::spawn_blocking(move || {
                let state = handle.state::<AppState>();
                if !state.is_active_notebook_epoch(&nb_id, active_epoch) {
                    return;
                }

                if let Err(e) = state.with_notebook_db(&nb_id, |_db| Ok(())) {
                    tracing::warn!(notebook_id = %nb_id, "Background notebook DB open failed: {}", e);
                    return;
                }
                if !state.is_active_notebook_epoch(&nb_id, active_epoch) {
                    return;
                }
            })
            .await;
        });
    }

    Ok(())
}
