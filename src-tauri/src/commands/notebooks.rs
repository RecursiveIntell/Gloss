use crate::db::app_db::Notebook;
use crate::db::doctor::{run_db_doctor, DbDoctorReceipt};
use crate::db::portable::{
    export_notebook_archive as export_notebook_archive_package, export_notebook_package,
    import_notebook_archive as import_notebook_archive_package, import_notebook_package,
    validate_notebook_archive, validate_notebook_package, NotebookExportReceipt,
    NotebookImportReceipt, NotebookPortableManifest,
};
use crate::error::GlossError;
use crate::jobs::{self, GlossJob};
use crate::state::AppState;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
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
pub async fn run_database_doctor(
    repair: bool,
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
) -> Result<DbDoctorReceipt, GlossError> {
    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    let mut receipt = run_db_doctor(&app_db, repair)?;
    let queue_report = inspect_queue_for_doctor(&app_db, &queue, repair)?;
    receipt.queue_jobs_checked = queue_report.checked;
    receipt.stale_queue_jobs = queue_report.stale;
    receipt.repaired_stale_queue_jobs = queue_report.repaired;
    Ok(receipt)
}

struct QueueDoctorReport {
    checked: usize,
    stale: usize,
    repaired: usize,
}

fn inspect_queue_for_doctor(
    app_db: &crate::db::app_db::AppDb,
    queue: &Arc<QueueManager>,
    repair: bool,
) -> Result<QueueDoctorReport, GlossError> {
    let notebooks = app_db.list_notebooks()?;
    let notebook_dirs = notebooks
        .iter()
        .map(|notebook| (notebook.id.clone(), PathBuf::from(&notebook.directory)))
        .collect::<HashMap<_, _>>();
    let mut source_cache: HashMap<String, HashSet<String>> = HashMap::new();
    let jobs = queue
        .list_jobs_with_data()
        .map_err(|e| GlossError::Other(format!("Failed to inspect queue jobs: {e}")))?;
    let mut checked = 0usize;
    let mut stale_job_ids = Vec::new();

    for (job_id, status, data_json) in jobs {
        if !matches!(status.as_str(), "pending" | "processing") {
            continue;
        }
        checked += 1;
        let Ok(job) = serde_json::from_str::<GlossJob>(&data_json) else {
            stale_job_ids.push(job_id);
            continue;
        };
        let Some(notebook_dir) = notebook_dirs.get(job.notebook_id()) else {
            stale_job_ids.push(job_id);
            continue;
        };
        let notebook_db_path = notebook_dir.join("notebook.db");
        if !notebook_db_path.exists() {
            stale_job_ids.push(job_id);
            continue;
        }
        if !source_cache.contains_key(job.notebook_id()) {
            let source_ids = crate::db::notebook_db::NotebookDb::open(&notebook_db_path)
                .and_then(|db| db.list_sources())
                .map(|sources| {
                    sources
                        .into_iter()
                        .map(|source| source.id)
                        .collect::<HashSet<_>>()
                })?;
            source_cache.insert(job.notebook_id().to_string(), source_ids);
        }
        if source_cache
            .get(job.notebook_id())
            .is_some_and(|sources| !sources.contains(job.source_id()))
        {
            stale_job_ids.push(job_id);
        }
    }

    let stale = stale_job_ids.len();
    let mut repaired = 0usize;
    if repair {
        for job_id in stale_job_ids {
            if queue.cancel(&job_id).is_ok() {
                repaired += 1;
            }
        }
    }

    Ok(QueueDoctorReport {
        checked,
        stale,
        repaired,
    })
}

#[tauri::command]
pub async fn export_notebook(
    notebook_id: String,
    package_dir: String,
    state: State<'_, AppState>,
) -> Result<NotebookExportReceipt, GlossError> {
    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    export_notebook_package(&app_db, &notebook_id, std::path::Path::new(&package_dir))
}

#[tauri::command]
pub async fn export_notebook_archive(
    notebook_id: String,
    archive_path: String,
    state: State<'_, AppState>,
) -> Result<NotebookExportReceipt, GlossError> {
    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    export_notebook_archive_package(&app_db, &notebook_id, std::path::Path::new(&archive_path))
}

#[tauri::command]
pub async fn validate_notebook_import_package(
    package_dir: String,
) -> Result<NotebookPortableManifest, GlossError> {
    validate_notebook_package(std::path::Path::new(&package_dir))
}

#[tauri::command]
pub async fn validate_notebook_import_archive(
    archive_path: String,
) -> Result<NotebookPortableManifest, GlossError> {
    validate_notebook_archive(std::path::Path::new(&archive_path))
}

#[tauri::command]
pub async fn import_notebook(
    package_dir: String,
    name_override: Option<String>,
    state: State<'_, AppState>,
) -> Result<NotebookImportReceipt, GlossError> {
    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    import_notebook_package(
        &app_db,
        std::path::Path::new(&package_dir),
        &state.data_dir.join("notebooks"),
        name_override.as_deref(),
    )
}

#[tauri::command]
pub async fn import_notebook_archive(
    archive_path: String,
    name_override: Option<String>,
    state: State<'_, AppState>,
) -> Result<NotebookImportReceipt, GlossError> {
    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    import_notebook_archive_package(
        &app_db,
        std::path::Path::new(&archive_path),
        &state.data_dir.join("notebooks"),
        name_override.as_deref(),
    )
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
pub async fn rename_notebook(
    id: String,
    name: String,
    state: State<'_, AppState>,
) -> Result<(), GlossError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(GlossError::Config("Notebook name cannot be empty".into()));
    }

    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    app_db.rename_notebook(&id, trimmed)?;
    Ok(())
}

fn notebook_owned_paths(
    data_dir: &std::path::Path,
    notebook_dir: &std::path::Path,
    notebook_id: &str,
) -> [std::path::PathBuf; 2] {
    [
        notebook_dir.to_path_buf(),
        crate::memory::semantic_memory_adapter::semantic_memory_base_dir(data_dir, notebook_id),
    ]
}

fn remove_rebuildable_notebook_projection(
    data_dir: &std::path::Path,
    notebook_id: &str,
) -> Result<(), GlossError> {
    let projection =
        crate::memory::semantic_memory_adapter::semantic_memory_base_dir(data_dir, notebook_id);
    if projection.exists() {
        std::fs::remove_dir_all(projection)?;
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_notebook(
    id: String,
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
) -> Result<(), GlossError> {
    // Resolve ownership before mutating runtime or registry state. The semantic
    // projection is rebuildable, so remove it first; failure leaves the
    // canonical notebook registered and retryable instead of orphaning it.
    let dir = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        app_db.get_notebook(&id)?.directory
    };
    let dir_path = std::path::PathBuf::from(&dir);
    let owned_paths = notebook_owned_paths(&state.data_dir, &dir_path, &id);
    remove_rebuildable_notebook_projection(&state.data_dir, &id)?;

    // If this is the active notebook, clear it and bump epoch so the summary
    // loop stops picking up jobs for it immediately.
    if state.get_active_notebook_id().as_deref() == Some(id.as_str()) {
        let _ = state.set_active_notebook(None, None);
    }

    let cancelled = jobs::cancel_jobs_matching(&queue, |job, _status| job.notebook_id() == id);
    if cancelled > 0 {
        tracing::info!(notebook_id = %id, cancelled, "Cancelled queued jobs for deleted notebook");
    }

    {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        app_db.delete_notebook(&id)?;
    }

    state.notebook_pools.remove(&id);

    {
        let mut indices = state
            .hnsw_indices
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        indices.remove(&id);
    }

    if owned_paths[0].exists() {
        std::fs::remove_dir_all(&owned_paths[0])?;
    }

    tracing::info!(id = %id, "Deleted notebook and rebuildable semantic projection");
    Ok(())
}

/// Set (or clear) the active notebook for scheduling purposes.
/// The summary worker will idle when no notebook is active.
/// Advances the epoch on real notebook switches so stale jobs are soft-cancelled,
/// but preserves the newest queued epoch for the selected notebook after app
/// restarts so pending work can resume instead of being discarded as stale.
#[tauri::command]
pub async fn set_active_notebook(
    notebook_id: Option<String>,
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
    app_handle: tauri::AppHandle,
) -> Result<(), GlossError> {
    // A rejected activation must leave the current scheduling owner unchanged.
    // touch_notebook also rejects a missing registry row.
    if let Some(ref nb_id) = notebook_id {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        app_db.touch_notebook(nb_id)?;
    }

    let next_epoch = notebook_id.as_deref().map(|nb_id| {
        let current_epoch = state.get_active_epoch();
        let resumed_epoch = jobs::max_pending_epoch_for_notebook(&queue, nb_id).unwrap_or(0);
        std::cmp::max(current_epoch.saturating_add(1), resumed_epoch)
    });

    let changed = state.set_active_notebook(notebook_id.clone(), next_epoch);
    if !changed {
        return Ok(());
    }

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
                }
            })
            .await;
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        inspect_queue_for_doctor, notebook_owned_paths, remove_rebuildable_notebook_projection,
    };
    use crate::db::app_db::AppDb;
    use crate::db::notebook_db::{NotebookDb, Source};
    use crate::jobs::GlossJob;
    use std::sync::Arc;
    use std::time::Duration;
    use tauri_queue::{QueueConfig, QueueJob, QueueManager};
    use tempfile::tempdir;

    fn source(id: &str) -> Source {
        Source {
            id: id.to_string(),
            source_type: "text".to_string(),
            title: id.to_string(),
            original_filename: None,
            file_hash: None,
            url: None,
            file_path: None,
            content_text: Some("content".to_string()),
            word_count: Some(1),
            metadata: None,
            summary: None,
            summary_model: None,
            status: "ready".to_string(),
            error_message: None,
            selected: true,
            created_at: String::new(),
            updated_at: String::new(),
            processing_state: None,
        }
    }

    #[test]
    fn notebook_owned_paths_exclude_the_application_model_cache() {
        let root = tempdir().unwrap();
        let notebook_dir = root.path().join("notebooks").join("nb1");
        let paths = notebook_owned_paths(root.path(), &notebook_dir, "nb1");
        assert_eq!(paths[0], notebook_dir);
        assert_eq!(paths[1], root.path().join("semantic-memory").join("nb1"));
        assert!(!paths.iter().any(|path| path == &root.path().join("models")));
    }

    #[test]
    fn notebook_projection_cleanup_removes_only_rebuildable_notebook_state() {
        let root = tempdir().unwrap();
        let projection = root.path().join("semantic-memory").join("nb1");
        let model_cache = root.path().join("models");
        std::fs::create_dir_all(&projection).unwrap();
        std::fs::create_dir_all(&model_cache).unwrap();
        std::fs::write(projection.join("memory.db"), b"projection").unwrap();
        std::fs::write(model_cache.join("model.bin"), b"model").unwrap();

        remove_rebuildable_notebook_projection(root.path(), "nb1").unwrap();

        assert!(!projection.exists());
        assert!(model_cache.join("model.bin").exists());
    }

    fn queue(dir: &tempfile::TempDir) -> Arc<QueueManager> {
        Arc::new(
            QueueManager::new(
                QueueConfig::builder()
                    .with_db_path(dir.path().join("queue.db"))
                    .with_poll_interval(Duration::from_secs(1))
                    .build(),
            )
            .unwrap(),
        )
    }

    #[test]
    fn db_doctor_cancels_stale_queue_jobs_for_missing_sources() {
        let dir = tempdir().unwrap();
        let app_db = AppDb::open(&dir.path().join("gloss.db")).unwrap();
        let notebook_dir = dir.path().join("notebooks").join("nb1");
        std::fs::create_dir_all(notebook_dir.join("sources")).unwrap();
        app_db
            .create_notebook("nb1", "Doctor", &notebook_dir.to_string_lossy())
            .unwrap();
        let notebook_db = NotebookDb::open(&notebook_dir.join("notebook.db")).unwrap();
        notebook_db.insert_source(&source("present")).unwrap();

        let queue = queue(&dir);
        queue
            .add(QueueJob::new(GlossJob::SummarizeSource {
                explicit_requested: false,
                epoch: 1,
                notebook_id: "nb1".to_string(),
                source_id: "missing".to_string(),
                source_title: "Missing".to_string(),
                data_dir: dir.path().to_string_lossy().to_string(),
                ollama_url: "http://localhost:11434".to_string(),
                model: "llama3".to_string(),
            }))
            .unwrap();
        queue
            .add(QueueJob::new(GlossJob::SummarizeSource {
                explicit_requested: false,
                epoch: 1,
                notebook_id: "nb1".to_string(),
                source_id: "present".to_string(),
                source_title: "Present".to_string(),
                data_dir: dir.path().to_string_lossy().to_string(),
                ollama_url: "http://localhost:11434".to_string(),
                model: "llama3".to_string(),
            }))
            .unwrap();

        let check = inspect_queue_for_doctor(&app_db, &queue, false).unwrap();
        assert_eq!(check.checked, 2);
        assert_eq!(check.stale, 1);
        assert_eq!(check.repaired, 0);

        let repair = inspect_queue_for_doctor(&app_db, &queue, true).unwrap();
        assert_eq!(repair.checked, 2);
        assert_eq!(repair.stale, 1);
        assert_eq!(repair.repaired, 1);
    }
}
