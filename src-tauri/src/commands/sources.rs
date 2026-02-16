use crate::db::notebook_db::{Chunk, NotebookStats, Source};
use crate::error::GlossError;
use crate::ingestion::chunk::chunk_text_with_title;
use crate::ingestion::extract::extract_text;
use crate::jobs::GlossJob;
use crate::state::AppState;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{Emitter, State};
use tauri_queue::{QueueJob, QueueManager, QueuePriority};

#[derive(Debug, Serialize)]
pub struct SourceContent {
    pub content_text: Option<String>,
    pub word_count: Option<i32>,
}

/// Classify a file extension into (source_type, optional language).
fn classify_extension(ext: &str) -> (&'static str, Option<&'static str>) {
    match ext {
        "txt" => ("text", None),
        "md" | "markdown" | "rst" => ("markdown", None),

        // Code files
        "py" => ("code", Some("python")),
        "js" => ("code", Some("javascript")),
        "jsx" => ("code", Some("jsx")),
        "ts" => ("code", Some("typescript")),
        "tsx" => ("code", Some("tsx")),
        "rs" => ("code", Some("rust")),
        "go" => ("code", Some("go")),
        "java" => ("code", Some("java")),
        "c" => ("code", Some("c")),
        "cpp" | "cc" | "cxx" => ("code", Some("cpp")),
        "h" | "hpp" => ("code", Some("c_header")),
        "cs" => ("code", Some("csharp")),
        "rb" => ("code", Some("ruby")),
        "php" => ("code", Some("php")),
        "swift" => ("code", Some("swift")),
        "kt" | "kts" => ("code", Some("kotlin")),
        "scala" => ("code", Some("scala")),
        "lua" => ("code", Some("lua")),
        "r" => ("code", Some("r")),
        "sql" => ("code", Some("sql")),
        "sh" | "bash" | "zsh" => ("code", Some("shell")),
        "css" => ("code", Some("css")),
        "scss" | "sass" => ("code", Some("scss")),
        "html" | "htm" => ("code", Some("html")),
        "xml" => ("code", Some("xml")),
        "json" => ("code", Some("json")),
        "yaml" | "yml" => ("code", Some("yaml")),
        "toml" => ("code", Some("toml")),
        "ini" | "cfg" | "conf" => ("code", Some("config")),
        "vue" => ("code", Some("vue")),
        "svelte" => ("code", Some("svelte")),
        "dart" => ("code", Some("dart")),
        "ex" | "exs" => ("code", Some("elixir")),
        "zig" => ("code", Some("zig")),
        "nim" => ("code", Some("nim")),
        "pl" | "pm" => ("code", Some("perl")),
        "proto" => ("code", Some("protobuf")),
        "graphql" | "gql" => ("code", Some("graphql")),
        "tf" | "hcl" => ("code", Some("terraform")),
        "dockerfile" => ("code", Some("dockerfile")),
        "makefile" => ("code", Some("makefile")),

        // Images
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "tiff" | "tif" => {
            ("image", None)
        }

        // Video
        "mp4" | "webm" | "mov" | "avi" | "mkv" => ("video", None),

        _ => ("text", None),
    }
}

/// Check if a file extension is supported for import.
fn is_supported_extension(ext: &str) -> bool {
    let (source_type, _) = classify_extension(ext);
    // All classified types are supported
    source_type != "text" || ext == "txt"
}

/// Create a file source record: copies file to notebook dir, inserts DB row.
/// Returns (source_id, source_type).
///
/// When `import_root` is provided (folder imports), the title uses the relative
/// path including the root folder name (e.g. `agent-graph/src/node.rs`) instead
/// of just the bare filename.
fn create_file_source(
    notebook_id: &str,
    source_path: &Path,
    import_root: Option<&Path>,
    state: &AppState,
) -> Result<(String, String), GlossError> {
    let filename = source_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    // For folder imports, use relative path as title
    // e.g., "agent-graph/src/node.rs" when importing ~/Coding/Libraries/
    let title = if let Some(root) = import_root {
        source_path
            .strip_prefix(root)
            .unwrap_or(source_path)
            .to_string_lossy()
            .to_string()
    } else {
        filename.clone()
    };

    let extension = source_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let (source_type, language) = classify_extension(&extension);

    // Read file and compute hash
    let content = std::fs::read(source_path).map_err(|e| GlossError::Ingestion {
        source_id: String::new(),
        message: format!("Failed to read file: {}", e),
    })?;
    let hash = format!("{:x}", Sha256::digest(&content));

    // Copy file to notebook sources directory
    let nb_dir = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let nb = app_db.get_notebook(notebook_id)?;
        PathBuf::from(nb.directory)
    };

    let dest_filename = format!("{}.{}", &hash[..16], extension);
    let dest_path = nb_dir.join("sources").join(&dest_filename);

    // Skip copy if destination already exists (dedup)
    if !dest_path.exists() {
        std::fs::copy(source_path, &dest_path)?;
    }

    // Build metadata JSON with language if present
    let metadata = language.map(|lang| {
        serde_json::json!({ "language": lang }).to_string()
    });

    // Create source record
    let source_id = uuid::Uuid::new_v4().to_string();
    let source = Source {
        id: source_id.clone(),
        source_type: source_type.to_string(),
        title,
        original_filename: Some(filename),
        file_hash: Some(hash),
        url: None,
        file_path: Some(dest_filename),
        content_text: None,
        word_count: None,
        metadata,
        summary: None,
        summary_model: None,
        status: "pending".to_string(),
        error_message: None,
        selected: true,
        created_at: String::new(),
        updated_at: String::new(),
    };

    state.with_notebook_db(notebook_id, |db| db.insert_source(&source))?;

    Ok((source_id, source_type.to_string()))
}

#[tauri::command]
pub async fn list_sources(
    notebook_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<Source>, GlossError> {
    state.with_notebook_db(&notebook_id, |db| db.list_sources())
}

/// Run the full ingestion pipeline: extract → chunk → embed.
/// After success, queues a background summary job if an LLM model is configured.
fn run_ingestion(
    notebook_id: &str,
    source_id: &str,
    state: &AppState,
    app_handle: &tauri::AppHandle,
    queue: &Arc<QueueManager>,
) {
    let result = (|| -> Result<(), GlossError> {
        // Get notebook dir + source record
        let nb_dir = {
            let app_db = state
                .app_db
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            let nb = app_db.get_notebook(notebook_id)?;
            PathBuf::from(nb.directory)
        };

        let source = state.with_notebook_db(notebook_id, |db| db.get_source(source_id))?;

        // 1. Extract text
        emit_status(app_handle, notebook_id, source_id, "extracting");
        let text = extract_text(&source, &nb_dir)?;
        let word_count = text.split_whitespace().count() as i32;
        state.with_notebook_db(notebook_id, |db| {
            db.update_source_content(source_id, &text, word_count)
        })?;

        // Skip chunking/embedding for non-text content (images, videos)
        if matches!(source.source_type.as_str(), "image" | "video") {
            state.with_notebook_db(notebook_id, |db| {
                db.update_source_status(source_id, "ready", None)
            })?;
            return Ok(());
        }

        // 2. Chunk (code-aware splitting for recognized extensions)
        emit_status(app_handle, notebook_id, source_id, "chunking");
        let chunks = chunk_text_with_title(&text, source_id, &source.title);
        tracing::debug!(source_id, chunks = chunks.len(), "Chunking complete");

        // 3. Insert chunks into DB
        state.with_notebook_db(notebook_id, |db| {
            for chunk_data in &chunks {
                let chunk = Chunk {
                    id: chunk_data.id.clone(),
                    source_id: source_id.to_string(),
                    chunk_index: chunk_data.chunk_index,
                    content: chunk_data.content.clone(),
                    token_count: chunk_data.token_count,
                    start_offset: chunk_data.start_offset,
                    end_offset: chunk_data.end_offset,
                    metadata: chunk_data.metadata.clone(),
                    embedding_id: None,
                    embedding_model: None,
                };
                db.insert_chunk(&chunk)?;
            }
            Ok(())
        })?;

        // 4. Embed chunks + add to HNSW
        emit_status(app_handle, notebook_id, source_id, "embedding");
        state.ensure_embedder(Some(app_handle))?;
        state.ensure_hnsw_index(notebook_id)?;

        let chunk_texts: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
        let embeddings = {
            let embedder = state
                .embedder
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            let embedder = embedder.as_ref().ok_or_else(|| {
                GlossError::Embedding("Embedder not initialized".into())
            })?;
            embedder.embed_batch(&chunk_texts)?
        };

        // Add vectors to HNSW index and update DB
        {
            let mut indices = state
                .hnsw_indices
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            let index = indices.get_mut(notebook_id).ok_or_else(|| {
                GlossError::Embedding("HNSW index not found".into())
            })?;

            state.with_notebook_db(notebook_id, |db| {
                for (i, chunk_data) in chunks.iter().enumerate() {
                    if let Some(embedding) = embeddings.get(i) {
                        let label = index.add(embedding)?;
                        db.update_chunk_embedding(
                            &chunk_data.id,
                            label as i64,
                            "NomicEmbedTextV15",
                        )?;
                    }
                }
                Ok(())
            })?;
        }

        // Save HNSW index to disk
        state.save_hnsw_index(notebook_id)?;

        // Mark ready
        state.with_notebook_db(notebook_id, |db| {
            db.update_source_status(source_id, "ready", None)
        })?;

        // Queue background summary job if a model is configured
        let source_title = source.title.clone();
        queue_summary_job(
            queue,
            state,
            notebook_id,
            source_id,
            &source_title,
        );

        tracing::info!(source_id, word_count, chunks = chunks.len(), "Ingestion complete");
        Ok(())
    })();

    let status = match &result {
        Ok(()) => "ready",
        Err(_) => "error",
    };

    if let Err(ref e) = result {
        tracing::warn!(source_id, error = %e, "Ingestion failed");
        let _ = state.with_notebook_db(notebook_id, |db| {
            db.update_source_status(source_id, "error", Some(&e.to_string()))
        });
    }

    emit_status(app_handle, notebook_id, source_id, status);
}

/// Queue a low-priority summary job for a source. Errors are logged but don't
/// fail the calling operation (summaries are non-critical background work).
pub(crate) fn queue_summary_job(
    queue: &Arc<QueueManager>,
    state: &AppState,
    notebook_id: &str,
    source_id: &str,
    source_title: &str,
) {
    // Get the model and Ollama URL from settings.
    // Prefer summary_model if set, otherwise fall back to default_model.
    let (ollama_url, model) = match (|| -> Result<(String, String), GlossError> {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let url = app_db
            .get_setting("ollama_url")?
            .unwrap_or_else(|| "http://localhost:11434".to_string());
        let model = app_db
            .get_setting("summary_model")?
            .filter(|m| !m.is_empty())
            .or_else(|| app_db.get_setting("default_model").ok().flatten());
        Ok((url, model.unwrap_or_default()))
    })() {
        Ok((url, model)) if !model.is_empty() => (url, model),
        _ => {
            tracing::debug!(source_id, "No model configured, skipping summary job");
            return;
        }
    };

    let job = QueueJob::new(GlossJob::SummarizeSource {
        notebook_id: notebook_id.to_string(),
        source_id: source_id.to_string(),
        source_title: source_title.to_string(),
        data_dir: state.data_dir.to_string_lossy().to_string(),
        ollama_url,
        model,
    })
    .with_priority(QueuePriority::Low);

    match queue.add(job) {
        Ok(job_id) => {
            tracing::debug!(source_id, job_id, "Queued summary job");
        }
        Err(e) => {
            tracing::warn!(source_id, error = %e, "Failed to queue summary job");
        }
    }
}

/// Emit a source status event to the frontend.
fn emit_status(
    app_handle: &tauri::AppHandle,
    notebook_id: &str,
    source_id: &str,
    status: &str,
) {
    let _ = app_handle.emit(
        "source:status",
        serde_json::json!({
            "notebook_id": notebook_id,
            "source_id": source_id,
            "status": status,
        }),
    );
}

#[tauri::command]
pub async fn add_source_file(
    notebook_id: String,
    path: String,
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
    app_handle: tauri::AppHandle,
) -> Result<String, GlossError> {
    let source_path = PathBuf::from(&path);
    let (source_id, _source_type) = create_file_source(&notebook_id, &source_path, None, &state)?;

    run_ingestion(&notebook_id, &source_id, &state, &app_handle, &queue);

    Ok(source_id)
}

/// Directories to skip when walking folders.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "__pycache__",
    ".git",
    "dist",
    "build",
    "vendor",
    ".venv",
    "venv",
    ".next",
    ".nuxt",
    ".cache",
];

/// Recursively walk a directory and collect supported file paths.
fn walk_directory(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), GlossError> {
    let entries = std::fs::read_dir(dir).map_err(|e| GlossError::Ingestion {
        source_id: String::new(),
        message: format!("Failed to read directory {}: {}", dir.display(), e),
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| GlossError::Ingestion {
            source_id: String::new(),
            message: format!("Failed to read entry: {}", e),
        })?;

        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip hidden files/dirs
        if name_str.starts_with('.') {
            continue;
        }

        if path.is_dir() {
            // Skip junk directories
            if SKIP_DIRS.contains(&name_str.as_ref()) {
                continue;
            }
            walk_directory(&path, out)?;
        } else if path.is_file() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            if is_supported_extension(&ext) {
                out.push(path);
            }
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn add_source_folder(
    notebook_id: String,
    path: String,
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
    app_handle: tauri::AppHandle,
) -> Result<Vec<String>, GlossError> {
    let folder = PathBuf::from(&path);
    if !folder.is_dir() {
        return Err(GlossError::Ingestion {
            source_id: String::new(),
            message: format!("Not a directory: {}", path),
        });
    }

    let mut files = Vec::new();
    walk_directory(&folder, &mut files)?;

    let mut source_ids = Vec::new();
    for file_path in &files {
        match create_file_source(&notebook_id, file_path, Some(&folder), &state) {
            Ok((source_id, _source_type)) => {
                run_ingestion(&notebook_id, &source_id, &state, &app_handle, &queue);
                source_ids.push(source_id);
            }
            Err(e) => {
                tracing::warn!(
                    file = %file_path.display(),
                    error = %e,
                    "Skipping file during folder import"
                );
            }
        }
    }

    tracing::info!(
        folder = %path,
        count = source_ids.len(),
        "Folder import complete"
    );

    Ok(source_ids)
}

#[tauri::command]
pub async fn add_source_paste(
    notebook_id: String,
    title: String,
    text: String,
    state: State<'_, AppState>,
) -> Result<String, GlossError> {
    let source_id = uuid::Uuid::new_v4().to_string();
    let word_count = text.split_whitespace().count() as i32;

    let source = Source {
        id: source_id.clone(),
        source_type: "paste".to_string(),
        title,
        original_filename: None,
        file_hash: None,
        url: None,
        file_path: None,
        content_text: Some(text),
        word_count: Some(word_count),
        metadata: None,
        summary: None,
        summary_model: None,
        status: "ready".to_string(),
        error_message: None,
        selected: true,
        created_at: String::new(),
        updated_at: String::new(),
    };

    state.with_notebook_db(&notebook_id, |db| db.insert_source(&source))?;

    Ok(source_id)
}

#[tauri::command]
pub async fn delete_source(
    notebook_id: String,
    source_id: String,
    state: State<'_, AppState>,
) -> Result<(), GlossError> {
    state.with_notebook_db(&notebook_id, |db| db.delete_source(&source_id))
}

#[tauri::command]
pub async fn get_source_content(
    notebook_id: String,
    source_id: String,
    state: State<'_, AppState>,
) -> Result<SourceContent, GlossError> {
    state.with_notebook_db(&notebook_id, |db| {
        let source = db.get_source(&source_id)?;
        Ok(SourceContent {
            content_text: source.content_text,
            word_count: source.word_count,
        })
    })
}

/// Retry ingestion for a failed source (Fix 9).
#[tauri::command]
pub async fn retry_source_ingestion(
    notebook_id: String,
    source_id: String,
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
    app_handle: tauri::AppHandle,
) -> Result<(), GlossError> {
    // Remove old vectors from HNSW index before deleting DB rows
    let old_embedding_ids: Vec<u64> = state.with_notebook_db(&notebook_id, |db| {
        db.get_embedding_ids_for_source(&source_id)
    })?;

    if !old_embedding_ids.is_empty() {
        let mut indices = state
            .hnsw_indices
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        if let Some(index) = indices.get_mut(&notebook_id) {
            for eid in &old_embedding_ids {
                let _ = index.remove(*eid);
            }
            tracing::debug!(count = old_embedding_ids.len(), source_id, "Removed old HNSW vectors");
        }
    }

    // Reset status and delete old chunks
    state.with_notebook_db(&notebook_id, |db| {
        db.update_source_status(&source_id, "pending", None)?;
        db.delete_chunks_for_source(&source_id)?;
        Ok(())
    })?;

    run_ingestion(&notebook_id, &source_id, &state, &app_handle, &queue);
    Ok(())
}

/// Get notebook-level statistics (Fix 10).
#[tauri::command]
pub async fn get_notebook_stats(
    notebook_id: String,
    state: State<'_, AppState>,
) -> Result<NotebookStats, GlossError> {
    state.with_notebook_db(&notebook_id, |db| db.get_stats())
}

/// Queue summary jobs for all sources that are ready but have no summary.
#[tauri::command]
pub async fn regenerate_missing_summaries(
    notebook_id: String,
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
) -> Result<u32, GlossError> {
    let count = auto_queue_notebook_summaries(&state, &queue, &notebook_id);
    Ok(count)
}

/// Auto-queue missing summaries for a notebook. Used by both the
/// `regenerate_missing_summaries` command and the summary_job_loop idle logic.
/// Returns the number of jobs queued.
///
/// Deduplication: if there are already pending or processing jobs in the queue,
/// skip queueing to prevent duplicate accumulation. The next call (after the
/// queue drains) will pick up any remaining unsummarized sources.
pub(crate) fn auto_queue_notebook_summaries(
    state: &AppState,
    queue: &Arc<QueueManager>,
    notebook_id: &str,
) -> u32 {
    // Dedup: don't queue more jobs if the queue already has pending/processing work.
    // This prevents the endlessly-climbing queue count on repeated calls from
    // startup, notebook switch, idle timer, and the "Generate" button.
    match queue.count_by_status() {
        Ok((pending, processing, _, _, _)) if pending + processing > 0 => {
            tracing::debug!(
                notebook_id,
                pending,
                processing,
                "Skipping auto-queue: jobs already pending"
            );
            return 0;
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to check queue status for dedup");
            // Fall through — better to risk duplicates than skip entirely
        }
        _ => {}
    }

    let sources = match state.with_notebook_db(notebook_id, |db| db.list_sources()) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(notebook_id, error = %e, "Failed to list sources for auto-queue");
            return 0;
        }
    };

    let mut count = 0u32;
    for source in &sources {
        if source.status == "ready" && source.summary.is_none() {
            queue_summary_job(queue, state, notebook_id, &source.id, &source.title);
            count += 1;
        }
    }

    if count > 0 {
        tracing::info!(notebook_id, count, "Auto-queued missing summary jobs");
    }
    count
}

#[derive(Debug, Serialize)]
pub struct QueueStatusResponse {
    pub paused: bool,
    pub pending: u32,
    pub processing: u32,
    pub completed: u32,
    pub failed: u32,
}

/// Pause background summary generation.
#[tauri::command]
pub async fn pause_summaries(state: State<'_, AppState>) -> Result<(), GlossError> {
    state.summary_paused.store(true, Ordering::SeqCst);
    tracing::info!("Summary generation paused by user");
    Ok(())
}

/// Resume background summary generation.
#[tauri::command]
pub async fn resume_summaries(state: State<'_, AppState>) -> Result<(), GlossError> {
    state.summary_paused.store(false, Ordering::SeqCst);
    tracing::info!("Summary generation resumed by user");
    Ok(())
}

/// Get the current queue status (paused state + job counts).
#[tauri::command]
pub async fn get_queue_status(
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
) -> Result<QueueStatusResponse, GlossError> {
    let paused = state.summary_paused.load(Ordering::SeqCst);
    let (pending, processing, completed, failed, _cancelled) = queue
        .count_by_status()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    Ok(QueueStatusResponse {
        paused,
        pending,
        processing,
        completed,
        failed,
    })
}
