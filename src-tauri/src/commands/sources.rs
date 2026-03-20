use crate::db::app_db::ModelRecord;
use crate::db::notebook_db::{Chunk, NotebookStats, Source};
use crate::error::GlossError;
use crate::ingestion::chunk::chunk_text_with_title;
use crate::ingestion::extract::extract_text;
use crate::jobs::{self, GlossJob};
use crate::state::{AppState, SUMMARY_MODE_AUTO, SUMMARY_MODE_MANUAL};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{Emitter, Manager, State};
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

        // SVG is text-based XML — treat as code
        "svg" => ("code", Some("xml")),

        // Images (vision pipeline)
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "tif" => ("image", None),

        // Video (Phase 3+)
        "mp4" | "webm" | "mov" | "avi" | "mkv" => ("video", None),

        // Treat unknown extensions as plain text
        _ => ("text", None),
    }
}

/// Binary/non-text extensions that should never be imported.
const BINARY_EXTENSIONS: &[&str] = &[
    // Compiled / object code
    "o",
    "obj",
    "so",
    "dll",
    "dylib",
    "a",
    "lib",
    "exe",
    "bin",
    "elf",
    "class",
    "pyc",
    "pyo",
    "wasm",
    // Archives
    "zip",
    "tar",
    "gz",
    "bz2",
    "xz",
    "7z",
    "rar",
    "zst",
    // Images: ico only (other image formats handled by vision pipeline)
    "ico",
    // Audio (not yet supported)
    "mp3",
    "wav",
    "ogg",
    "flac",
    "m4a",
    "aac",
    "wma",
    // Fonts
    "ttf",
    "otf",
    "woff",
    "woff2",
    "eot",
    // Documents (Phase 2+)
    "pdf",
    "docx",
    "doc",
    "xlsx",
    "xls",
    "pptx",
    "ppt",
    // Database / data
    "db",
    "sqlite",
    "sqlite3",
    "mdb",
    // OS / misc binary
    "DS_Store",
    "swp",
    "swo",
    // ONNX / ML models
    "onnx",
    "pt",
    "pth",
    "safetensors",
    "gguf",
    "ggml",
    // Usearch index
    "usearch",
    // Lock files (often huge, not useful as source content)
    "lock",
];

/// Check if a file extension is supported for import.
fn is_supported_extension(ext: &str) -> bool {
    if ext.is_empty() {
        // Files without extensions (Makefile, Dockerfile, LICENSE, etc.)
        return true;
    }
    !BINARY_EXTENSIONS.contains(&ext)
}

fn hash_file(path: &Path) -> Result<String, GlossError> {
    let file = File::open(path).map_err(|e| GlossError::Ingestion {
        source_id: String::new(),
        message: format!("Failed to open file: {}", e),
    })?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];

    loop {
        let read = reader.read(&mut buf).map_err(|e| GlossError::Ingestion {
            source_id: String::new(),
            message: format!("Failed to read file: {}", e),
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }

    Ok(format!("{:x}", hasher.finalize()))
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

    // Stream the file through the hasher so large media files do not allocate
    // their full contents in memory during folder import.
    let hash = hash_file(source_path)?;

    // Check for existing source with same hash (dedup on re-import)
    let existing = state.with_notebook_db(notebook_id, |db| db.source_exists_by_hash(&hash))?;
    if existing {
        tracing::debug!(file = %source_path.display(), "Skipping duplicate (hash match)");
        return Err(GlossError::Ingestion {
            source_id: String::new(),
            message: "duplicate".to_string(),
        });
    }

    // Copy file to notebook sources directory
    let nb_dir = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let nb = app_db.get_notebook(notebook_id)?;
        PathBuf::from(nb.directory)
    };

    let dest_filename = if extension.is_empty() {
        hash[..16].to_string()
    } else {
        format!("{}.{}", &hash[..16], extension)
    };
    let dest_path = nb_dir.join("sources").join(&dest_filename);

    // Skip copy if destination already exists (dedup)
    if !dest_path.exists() {
        std::fs::copy(source_path, &dest_path)?;
    }

    // Build metadata JSON with language if present
    let metadata = language.map(|lang| serde_json::json!({ "language": lang }).to_string());

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
    sync_notebook_source_count(state, notebook_id)?;

    Ok((source_id, source_type.to_string()))
}

#[tauri::command]
pub async fn list_sources(
    notebook_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<Source>, GlossError> {
    state.with_notebook_db(&notebook_id, |db| db.list_sources())
}

#[tauri::command]
pub async fn set_selected_sources(
    notebook_id: String,
    selected_source_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<(), GlossError> {
    state.with_notebook_db(&notebook_id, |db| {
        db.set_selected_sources(&selected_source_ids)
    })?;
    invalidate_suggested_questions(&state, &notebook_id);
    Ok(())
}

/// Options for controlling ingestion behavior during batch imports.
struct IngestionOpts {
    /// Save HNSW index to disk after this source
    save_index: bool,
    /// Run semantic embedding + HNSW indexing for this source.
    /// Folder imports disable this to avoid crashing the desktop process inside
    /// native ONNX/usearch code; chat falls back to DB chunks for such sources.
    embed_chunks: bool,
    /// Queue a background summary job after ingestion
    queue_summary: bool,
    /// Emit intermediate status events (extracting, chunking, embedding).
    emit_progress: bool,
    /// Emit the terminal ready/error event to the frontend.
    /// Batch folder imports suppress these per-source events and rely on the
    /// final list refresh instead to avoid flooding the renderer.
    emit_final_status: bool,
}

impl Default for IngestionOpts {
    fn default() -> Self {
        Self {
            save_index: true,
            embed_chunks: crate::state::NATIVE_SEMANTIC_INDEXING_ENABLED,
            queue_summary: true,
            emit_progress: true,
            emit_final_status: true,
        }
    }
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
    run_ingestion_inner(
        notebook_id,
        source_id,
        state,
        app_handle,
        queue,
        IngestionOpts::default(),
    );
}

fn run_ingestion_inner(
    notebook_id: &str,
    source_id: &str,
    state: &AppState,
    app_handle: &tauri::AppHandle,
    queue: &Arc<QueueManager>,
    opts: IngestionOpts,
) {
    // Signal that ingestion is active so the summary loop yields
    state.ingestion_active.fetch_add(1, Ordering::SeqCst);

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
        if opts.emit_progress {
            emit_status(app_handle, notebook_id, source_id, "extracting", None);
        }
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
        if opts.emit_progress {
            emit_status(app_handle, notebook_id, source_id, "chunking", None);
        }
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

        if opts.embed_chunks {
            // Acquire GPU gate to prevent ONNX + Ollama VRAM contention
            if opts.emit_progress {
                emit_status(app_handle, notebook_id, source_id, "waiting_for_gpu", None);
            }
            let _gpu_permit = loop {
                match state.gpu_gate.try_acquire() {
                    Ok(permit) => break permit,
                    Err(tokio::sync::TryAcquireError::NoPermits) => {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    Err(tokio::sync::TryAcquireError::Closed) => {
                        return Err(GlossError::Other(
                            "GPU gate closed — app shutting down".into(),
                        ));
                    }
                }
            };

            // 4. Embed chunks + add to HNSW
            if opts.emit_progress {
                emit_status(app_handle, notebook_id, source_id, "embedding", None);
            }
            state.ensure_embedder(Some(app_handle))?;
            state.ensure_hnsw_index(notebook_id)?;

            let chunk_texts: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
            let embeddings = {
                let embedder = state
                    .embedder
                    .lock()
                    .map_err(|e| GlossError::Other(e.to_string()))?;
                let embedder = embedder
                    .as_ref()
                    .ok_or_else(|| GlossError::Embedding("Embedder not initialized".into()))?;
                embedder.embed_batch(&chunk_texts)?
            };

            // Add vectors to HNSW index and update DB
            {
                let mut indices = state
                    .hnsw_indices
                    .lock()
                    .map_err(|e| GlossError::Other(e.to_string()))?;
                let index = indices
                    .get_mut(notebook_id)
                    .ok_or_else(|| GlossError::Embedding("HNSW index not found".into()))?;

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

            // Save HNSW index to disk (skipped during batch imports)
            if opts.save_index {
                state.save_hnsw_index(notebook_id)?;
            }
        }

        // Mark ready
        state.with_notebook_db(notebook_id, |db| {
            db.update_source_status(source_id, "ready", None)
        })?;

        // Queue background summary job if a model is configured
        if opts.queue_summary {
            let source_title = source.title.clone();
            if let Err(error) =
                queue_summary_job(queue, state, notebook_id, source_id, &source_title)
            {
                tracing::warn!(source_id, error = %error, "Failed to schedule background summary job");
            }
        }

        tracing::info!(
            source_id,
            word_count,
            chunks = chunks.len(),
            "Ingestion complete"
        );
        Ok(())
    })();

    // Decrement active ingestion counter (always, even on error)
    state.ingestion_active.fetch_sub(1, Ordering::SeqCst);

    if let Err(e) = &result {
        if is_deleted_source_error(e, source_id) {
            tracing::info!(
                notebook_id,
                source_id,
                "Source deleted during ingestion, stopping quietly"
            );
            return;
        }
    }

    let (status, error_msg) = match &result {
        Ok(()) => ("ready", None),
        Err(e) => ("error", Some(e.to_string())),
    };

    if let Some(ref msg) = error_msg {
        tracing::warn!(source_id, error = %msg, "Ingestion failed");
        let _ = state.with_notebook_db(notebook_id, |db| {
            db.update_source_status(source_id, "error", Some(msg))
        });
    }

    if opts.emit_final_status {
        emit_status(
            app_handle,
            notebook_id,
            source_id,
            status,
            error_msg.as_deref(),
        );
    }
}

fn queue_epoch_for_notebook(state: &AppState, notebook_id: &str) -> u64 {
    if state.get_active_notebook_id().as_deref() == Some(notebook_id) {
        state.get_active_epoch()
    } else {
        0
    }
}

fn is_deleted_source_error(err: &GlossError, source_id: &str) -> bool {
    matches!(err, GlossError::NotFound(message) if message.contains(&format!("Source {source_id} not found")))
}

#[derive(Clone, Copy)]
enum BackgroundJobKind {
    Summary,
    Vision,
}

impl BackgroundJobKind {
    fn setting_key(self) -> &'static str {
        match self {
            BackgroundJobKind::Summary => "summary_model",
            BackgroundJobKind::Vision => "vision_model",
        }
    }

    fn label(self) -> &'static str {
        match self {
            BackgroundJobKind::Summary => "summary",
            BackgroundJobKind::Vision => "vision",
        }
    }
}

#[derive(Debug, Clone)]
struct BackgroundJobConfig {
    provider_id: String,
    base_url: String,
    model: String,
}

#[derive(Debug, Serialize)]
pub struct QueueSummariesResponse {
    pub queued: u32,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct BackgroundBackendStatus {
    pub ready: bool,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub diagnostic: Option<String>,
}

fn normalize_setting(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn has_vision_capability(model: &ModelRecord) -> bool {
    if let Some(capabilities) = model.capabilities.as_deref() {
        if capabilities
            .split(',')
            .map(|cap| cap.trim().to_ascii_lowercase())
            .any(|cap| matches!(cap.as_str(), "vision" | "image" | "multimodal"))
        {
            return true;
        }
    }

    let fingerprint = format!(
        "{} {}",
        model.id.to_ascii_lowercase(),
        model.display_name.to_ascii_lowercase()
    );
    [
        "llava",
        "bakllava",
        "moondream",
        "minicpm-v",
        "qwen-vl",
        "qwen2-vl",
        "qwen2.5-vl",
        "vision",
        "vl",
    ]
    .iter()
    .any(|needle| fingerprint.contains(needle))
}

fn resolve_background_job_config(
    state: &AppState,
    kind: BackgroundJobKind,
) -> Result<BackgroundJobConfig, String> {
    let app_db = state.app_db.lock().map_err(|e| e.to_string())?;
    resolve_background_job_config_from_db(&app_db, kind)
}

fn resolve_background_job_config_from_db(
    app_db: &crate::db::app_db::AppDb,
    kind: BackgroundJobKind,
) -> Result<BackgroundJobConfig, String> {
    let configured_model = normalize_setting(
        app_db
            .get_setting(kind.setting_key())
            .map_err(|e| e.to_string())?,
    );
    let default_model = normalize_setting(
        app_db
            .get_setting("default_model")
            .map_err(|e| e.to_string())?,
    );
    let selected_model = configured_model
        .clone()
        .or(default_model.clone())
        .ok_or_else(|| {
            format!(
                "No {} model is configured. Choose a compatible Ollama model in Settings.",
                kind.label()
            )
        })?;

    let models = app_db.get_all_models().map_err(|e| e.to_string())?;
    let model_record = models.iter().find(|model| model.id == selected_model);
    let provider_id = model_record
        .map(|model| model.provider_id.clone())
        .or_else(|| {
            if configured_model.is_none() && default_model.as_deref() == Some(selected_model.as_str())
            {
                normalize_setting(app_db.get_setting("default_provider").ok().flatten())
            } else {
                None
            }
        })
        .ok_or_else(|| {
            format!(
                "Background {} model '{}' is not available in the model registry. Refresh models or choose a compatible Ollama model.",
                kind.label(),
                selected_model
            )
        })?;

    if provider_id != "ollama" {
        return Err(format!(
            "Background {} requires an Ollama model, but '{}' uses provider '{}'.",
            kind.label(),
            selected_model,
            provider_id
        ));
    }

    if matches!(kind, BackgroundJobKind::Vision) {
        match model_record {
            Some(model) if has_vision_capability(model) => {}
            Some(_) | None => {
                return Err(format!(
                    "Background vision requires a vision-capable Ollama model, but '{}' is not marked as vision-capable.",
                    selected_model
                ));
            }
        }
    }

    let base_url = app_db
        .get_setting("ollama_url")
        .map_err(|e| e.to_string())?
        .unwrap_or_else(|| "http://localhost:11434".to_string());

    Ok(BackgroundJobConfig {
        provider_id,
        base_url,
        model: selected_model,
    })
}

fn background_backend_status(state: &AppState, kind: BackgroundJobKind) -> BackgroundBackendStatus {
    match resolve_background_job_config(state, kind) {
        Ok(config) => BackgroundBackendStatus {
            ready: true,
            provider_id: Some(config.provider_id),
            model: Some(config.model),
            diagnostic: None,
        },
        Err(diagnostic) => BackgroundBackendStatus {
            ready: false,
            provider_id: None,
            model: None,
            diagnostic: Some(diagnostic),
        },
    }
}

fn persist_summary_mode(state: &AppState, mode: &str) -> Result<(), GlossError> {
    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    app_db.set_setting("summary_mode", mode)
}

fn push_diagnostic(diagnostics: &mut Vec<String>, diagnostic: String) {
    if diagnostics.iter().all(|existing| existing != &diagnostic) {
        diagnostics.push(diagnostic);
    }
}

/// Queue a low-priority summary job for a source. Errors are surfaced to the
/// caller so queue counts stay truthful.
pub(crate) fn queue_summary_job(
    queue: &Arc<QueueManager>,
    state: &AppState,
    notebook_id: &str,
    source_id: &str,
    source_title: &str,
) -> Result<bool, String> {
    let config = resolve_background_job_config(state, BackgroundJobKind::Summary)?;

    let job = QueueJob::new(GlossJob::SummarizeSource {
        epoch: queue_epoch_for_notebook(state, notebook_id),
        notebook_id: notebook_id.to_string(),
        source_id: source_id.to_string(),
        source_title: source_title.to_string(),
        data_dir: state.data_dir.to_string_lossy().to_string(),
        ollama_url: config.base_url,
        model: config.model,
    })
    .with_priority(QueuePriority::Low);

    match queue.add(job) {
        Ok(job_id) => {
            tracing::debug!(source_id, job_id, "Queued summary job");
            Ok(true)
        }
        Err(e) => Err(format!("Failed to queue summary job: {e}")),
    }
}

/// Queue a describe-image job for an image source. Looks up the vision model
/// from settings (falls back to default_model if unset).
fn queue_describe_image_job(
    queue: &Arc<QueueManager>,
    state: &AppState,
    notebook_id: &str,
    source_id: &str,
    _source_path: &Path,
) -> Result<(), String> {
    let config = match resolve_background_job_config(state, BackgroundJobKind::Vision) {
        Ok(config) => config,
        Err(msg) => {
            tracing::warn!(source_id, "{msg}");
            let _ = state.with_notebook_db(notebook_id, |db| {
                db.update_source_status(source_id, "error", Some(&msg))
            });
            return Err(msg);
        }
    };

    let source_title = state
        .with_notebook_db(notebook_id, |db| db.get_source(source_id).map(|s| s.title))
        .unwrap_or_else(|_| source_id.to_string());

    let job = QueueJob::new(GlossJob::DescribeImage {
        epoch: queue_epoch_for_notebook(state, notebook_id),
        notebook_id: notebook_id.to_string(),
        source_id: source_id.to_string(),
        source_title,
        data_dir: state.data_dir.to_string_lossy().to_string(),
        ollama_url: config.base_url,
        model: config.model,
    })
    .with_priority(QueuePriority::Low);

    match queue.add(job) {
        Ok(job_id) => {
            tracing::info!(source_id, job_id, "Queued describe-image job");
            Ok(())
        }
        Err(e) => {
            let msg = format!("Failed to queue image description job: {e}");
            tracing::warn!(source_id, error = %e, "Failed to queue describe-image job");
            let _ = state.with_notebook_db(notebook_id, |db| {
                db.update_source_status(source_id, "error", Some(&msg))
            });
            Err(msg)
        }
    }
}

/// Queue a describe-video job for a video source. Uses the vision model to
/// describe extracted frames. Requires ffmpeg to be installed.
fn queue_describe_video_job(
    queue: &Arc<QueueManager>,
    state: &AppState,
    notebook_id: &str,
    source_id: &str,
) -> Result<(), String> {
    let config = match resolve_background_job_config(state, BackgroundJobKind::Vision) {
        Ok(config) => config,
        Err(msg) => {
            tracing::warn!(source_id, "{msg}");
            let _ = state.with_notebook_db(notebook_id, |db| {
                db.update_source_status(source_id, "error", Some(&msg))
            });
            return Err(msg);
        }
    };

    let source_title = state
        .with_notebook_db(notebook_id, |db| db.get_source(source_id).map(|s| s.title))
        .unwrap_or_else(|_| source_id.to_string());

    let job = QueueJob::new(GlossJob::DescribeVideo {
        epoch: queue_epoch_for_notebook(state, notebook_id),
        notebook_id: notebook_id.to_string(),
        source_id: source_id.to_string(),
        source_title,
        data_dir: state.data_dir.to_string_lossy().to_string(),
        ollama_url: config.base_url,
        model: config.model,
    })
    .with_priority(QueuePriority::Low);

    match queue.add(job) {
        Ok(job_id) => {
            tracing::info!(source_id, job_id, "Queued describe-video job");
            Ok(())
        }
        Err(e) => {
            let msg = format!("Failed to queue video description job: {e}");
            tracing::warn!(source_id, error = %e, "Failed to queue describe-video job");
            let _ = state.with_notebook_db(notebook_id, |db| {
                db.update_source_status(source_id, "error", Some(&msg))
            });
            Err(msg)
        }
    }
}

/// Run the embedding pipeline for a described source (image/video that already
/// has content_text and chunks but no vector embeddings).
pub(crate) fn embed_described_source(
    state: &AppState,
    notebook_id: &str,
    source_id: &str,
    app_handle: &tauri::AppHandle,
    queue: &Arc<QueueManager>,
) {
    let result = (|| -> Result<(), GlossError> {
        let chunks: Vec<crate::db::notebook_db::Chunk> =
            state.with_notebook_db(notebook_id, |db| db.get_chunks_for_source(source_id))?;
        state.with_notebook_db(notebook_id, |db| {
            db.update_source_status(source_id, "ready", None)
        })?;

        // Queue summary job
        let source_title = state
            .with_notebook_db(notebook_id, |db| db.get_source(source_id).map(|s| s.title))
            .unwrap_or_else(|_| source_id.to_string());
        if let Err(error) = queue_summary_job(queue, state, notebook_id, source_id, &source_title) {
            tracing::warn!(source_id, error = %error, "Failed to schedule summary for described source");
        }

        tracing::info!(
            source_id,
            chunks = chunks.len(),
            "Marked described source ready without semantic embeddings"
        );
        Ok(())
    })();

    let embed_error_msg = if let Err(ref e) = result {
        tracing::warn!(source_id, error = %e, "Embedding failed for described source");
        let msg = e.to_string();
        let _ = state.with_notebook_db(notebook_id, |db| {
            db.update_source_status(source_id, "error", Some(&msg))
        });
        Some(msg)
    } else {
        None
    };

    emit_status(
        app_handle,
        notebook_id,
        source_id,
        if result.is_ok() { "ready" } else { "error" },
        embed_error_msg.as_deref(),
    );
}

/// Emit a source status event to the frontend.
fn emit_status(
    app_handle: &tauri::AppHandle,
    notebook_id: &str,
    source_id: &str,
    status: &str,
    error_message: Option<&str>,
) {
    let mut payload = serde_json::json!({
        "notebook_id": notebook_id,
        "source_id": source_id,
        "status": status,
    });
    if let Some(msg) = error_message {
        payload["error_message"] = serde_json::json!(msg);
    }
    let _ = app_handle.emit("source:status", payload);
}

fn invalidate_suggested_questions(state: &AppState, notebook_id: &str) {
    if let Err(e) =
        state.with_notebook_db(notebook_id, |db| db.set_config("suggested_questions", ""))
    {
        tracing::warn!(notebook_id, error = %e, "Failed to invalidate suggested questions cache");
    }
}

fn sync_notebook_source_count(state: &AppState, notebook_id: &str) -> Result<(), GlossError> {
    let count = state.with_notebook_db(notebook_id, |db| db.source_count())?;
    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    app_db.update_source_count(notebook_id, count)?;
    Ok(())
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

    // Check file size before creating source
    let ext = source_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let (pre_source_type, _) = classify_extension(&ext);
    if let Ok(meta) = source_path.metadata() {
        let limit = match pre_source_type {
            "image" => MAX_IMAGE_FILE_SIZE,
            "video" => MAX_VIDEO_FILE_SIZE,
            _ => MAX_IMPORT_FILE_SIZE,
        };
        if meta.len() > limit {
            return Err(GlossError::Ingestion {
                source_id: String::new(),
                message: format!(
                    "File too large ({:.1} MB, max {} MB)",
                    meta.len() as f64 / (1024.0 * 1024.0),
                    limit / (1024 * 1024)
                ),
            });
        }
    }

    let (source_id, source_type) = create_file_source(&notebook_id, &source_path, None, &state)?;
    invalidate_suggested_questions(&state, &notebook_id);

    match source_type.as_str() {
        "image" => {
            match queue_describe_image_job(&queue, &state, &notebook_id, &source_id, &source_path) {
                Ok(()) => emit_status(&app_handle, &notebook_id, &source_id, "pending", None),
                Err(msg) => emit_status(&app_handle, &notebook_id, &source_id, "error", Some(&msg)),
            }
        }
        "video" => match queue_describe_video_job(&queue, &state, &notebook_id, &source_id) {
            Ok(()) => emit_status(&app_handle, &notebook_id, &source_id, "pending", None),
            Err(msg) => emit_status(&app_handle, &notebook_id, &source_id, "error", Some(&msg)),
        },
        _ => {
            // Spawn ingestion in background so the IPC thread isn't blocked
            let nb = notebook_id.clone();
            let src = source_id.clone();
            let q = Arc::clone(&queue);
            let handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                let _ = tokio::task::spawn_blocking(move || {
                    let state = handle.state::<AppState>();
                    run_ingestion(&nb, &src, &state, &handle, &q);
                })
                .await;
            });
        }
    }

    Ok(source_id)
}

/// Maximum file size (10 MB) for folder imports. Files larger than this are
/// skipped to prevent OOM from reading/hashing/embedding huge files.
const MAX_IMPORT_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// Maximum image file size (10 MB) for vision pipeline.
const MAX_IMAGE_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// Maximum video file size (100 MB) for frame analysis pipeline.
const MAX_VIDEO_FILE_SIZE: u64 = 100 * 1024 * 1024;

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

/// Maximum directory depth to traverse.
const MAX_WALK_DEPTH: usize = 20;
/// Maximum number of files to collect from a single folder import.
const MAX_WALK_FILES: usize = 5000;
/// Number of source records to create per batch before emitting a refresh
/// signal during long folder imports.
const SOURCE_CREATION_BATCH_SIZE: usize = 25;

/// Recursively walk a directory and collect supported file paths.
/// Skips symlinks, hidden files, and known junk directories.
/// Stops early if depth or file count limits are reached.
fn walk_directory(dir: &Path, out: &mut Vec<PathBuf>) {
    walk_directory_inner(dir, out, 0);
}

fn walk_directory_inner(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > MAX_WALK_DEPTH || out.len() >= MAX_WALK_FILES {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(dir = %dir.display(), error = %e, "Cannot read directory, skipping");
            return;
        }
    };

    for entry in entries {
        if out.len() >= MAX_WALK_FILES {
            tracing::warn!("File limit ({}) reached during folder walk", MAX_WALK_FILES);
            return;
        }

        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(dir = %dir.display(), error = %e, "Bad directory entry, skipping");
                continue;
            }
        };

        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip hidden files/dirs
        if name_str.starts_with('.') {
            continue;
        }

        // Use symlink_metadata to detect symlinks without following them
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        // Skip symlinks entirely to avoid loops
        if meta.file_type().is_symlink() {
            tracing::debug!(path = %path.display(), "Skipping symlink");
            continue;
        }

        if meta.is_dir() {
            if SKIP_DIRS.contains(&name_str.as_ref()) {
                tracing::debug!(dir = %name_str, "Skipping excluded directory");
                continue;
            }
            walk_directory_inner(&path, out, depth + 1);
        } else if meta.is_file() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            if is_supported_extension(&ext) {
                out.push(path);
            } else {
                tracing::debug!(file = %path.display(), ext = %ext, "Skipping unsupported file type");
            }
        }
    }
}

#[tauri::command]
pub async fn add_source_folder(
    notebook_id: String,
    path: String,
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
    app_handle: tauri::AppHandle,
) -> Result<(), GlossError> {
    invalidate_suggested_questions(&state, &notebook_id);
    let folder = PathBuf::from(&path);
    if !folder.is_dir() {
        return Err(GlossError::Ingestion {
            source_id: String::new(),
            message: format!("Not a directory: {}", path),
        });
    }

    let nb_id = notebook_id.clone();
    let q = Arc::clone(&queue);
    let handle = app_handle.clone();
    let folder_walk = folder.clone();
    let folder_create = folder.clone();
    let import_path = path.clone();
    tauri::async_runtime::spawn(async move {
        // Keep the IPC command fast: scan the folder entirely in the background.
        let files = match tokio::task::spawn_blocking(move || {
            let mut files = Vec::new();
            walk_directory(&folder_walk, &mut files);
            files
        })
        .await
        {
            Ok(files) => files,
            Err(e) => {
                tracing::error!(
                    folder = %import_path,
                    error = %e,
                    "Directory walk failed during background folder import"
                );
                return;
            }
        };

        if files.len() >= MAX_WALK_FILES {
            tracing::warn!(
                folder = %import_path,
                "Folder contains more than {} supported files; only importing first {}",
                MAX_WALK_FILES, MAX_WALK_FILES
            );
        }

        let file_count = files.len();
        tracing::info!(
            folder = %import_path,
            files_found = file_count,
            "Directory walk complete"
        );

        if file_count == 0 {
            return;
        }

        // Phase 1: Create source records (blocking I/O — file reads + copies)
        let mut sources: Vec<(String, String)> = Vec::new();
        for batch in files.chunks(SOURCE_CREATION_BATCH_SIZE) {
            let batch_files = batch.to_vec();
            let handle_p1 = handle.clone();
            let nb_id_p1 = nb_id.clone();
            let folder_p1 = folder_create.clone();
            let created = tokio::task::spawn_blocking(move || {
                let state = handle_p1.state::<AppState>();
                let mut created: Vec<(String, String)> = Vec::new();
                for file_path in batch_files {
                    if let Ok(meta) = file_path.metadata() {
                        let ext = file_path
                            .extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or("")
                            .to_lowercase();
                        let (stype, _) = classify_extension(&ext);
                        let limit = match stype {
                            "image" => MAX_IMAGE_FILE_SIZE,
                            "video" => MAX_VIDEO_FILE_SIZE,
                            _ => MAX_IMPORT_FILE_SIZE,
                        };
                        if meta.len() > limit {
                            tracing::warn!(
                                file = %file_path.display(),
                                size_mb = meta.len() / (1024 * 1024),
                                "Skipping oversized file (>{} MB)",
                                limit / (1024 * 1024)
                            );
                            continue;
                        }
                    }

                    match create_file_source(&nb_id_p1, &file_path, Some(&folder_p1), &state) {
                        Ok((source_id, source_type)) => created.push((source_id, source_type)),
                        Err(GlossError::Ingestion { message, .. }) if message == "duplicate" => {
                            // Silently skip duplicates on re-import
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
                created
            })
            .await
            .unwrap_or_default();

            if created.is_empty() {
                continue;
            }

            sources.extend(created);
            let _ = handle.emit(
                "sources:batch_created",
                serde_json::json!({
                    "notebook_id": &nb_id,
                    "count": sources.len(),
                }),
            );

            tokio::task::yield_now().await;
        }

        let total = sources.len();
        if total == 0 {
            return;
        }

        // Hold ingestion_active for the entire batch so the summary loop
        // cannot steal the GPU gate between individual source ingestions.
        {
            let state = handle.state::<AppState>();
            state.ingestion_active.fetch_add(1, Ordering::SeqCst);
        }

        // Phase 2: Ingest one at a time on blocking thread pool.
        // spawn_blocking isolates panics from ONNX/usearch C++ code.
        // Images are routed to the vision pipeline (queued jobs) instead.
        for (i, (source_id, source_type)) in sources.into_iter().enumerate() {
            let nb_id = nb_id.clone();
            let q = q.clone();
            let handle = handle.clone();
            let result = tokio::task::spawn_blocking(move || {
                let state = handle.state::<AppState>();
                match source_type.as_str() {
                    "image" => {
                        let _ =
                            queue_describe_image_job(&q, &state, &nb_id, &source_id, Path::new(""));
                    }
                    "video" => {
                        let _ = queue_describe_video_job(&q, &state, &nb_id, &source_id);
                    }
                    _ => {
                        run_ingestion_inner(
                            &nb_id,
                            &source_id,
                            &state,
                            &handle,
                            &q,
                            IngestionOpts {
                                save_index: false,
                                embed_chunks: false,
                                queue_summary: false,
                                emit_progress: false,
                                emit_final_status: false,
                            },
                        );
                    }
                }
            })
            .await;

            if let Err(e) = result {
                tracing::error!(
                    index = i,
                    total,
                    error = %e,
                    "Ingestion task panicked, continuing with remaining files"
                );
            }

            // Brief pause between sources to let GPU memory settle
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // Release the batch-level ingestion hold
        {
            let state = handle.state::<AppState>();
            state.ingestion_active.fetch_sub(1, Ordering::SeqCst);
        }

        // Save HNSW index once after all sources are ingested
        {
            let handle_save = handle.clone();
            let nb_save = nb_id.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let state = handle_save.state::<AppState>();
                if let Err(e) = state.save_hnsw_index(&nb_save) {
                    tracing::error!(error = %e, "Failed to save HNSW index after batch import");
                }
            })
            .await;
        }

        // Notify frontend that batch ingestion is complete
        let _ = handle.emit(
            "sources:batch_ingestion_complete",
            serde_json::json!({
                "notebook_id": &nb_id,
                "count": total,
            }),
        );

        tracing::info!(
            folder = %import_path,
            count = total,
            "Background folder ingestion complete"
        );
    });

    tracing::info!(folder = %path, "Folder import scheduled in background");
    Ok(())
}

#[tauri::command]
pub async fn add_source_paste(
    notebook_id: String,
    title: String,
    text: String,
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
    app_handle: tauri::AppHandle,
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
        status: "pending".to_string(),
        error_message: None,
        selected: true,
        created_at: String::new(),
        updated_at: String::new(),
    };

    state.with_notebook_db(&notebook_id, |db| db.insert_source(&source))?;
    sync_notebook_source_count(&state, &notebook_id)?;
    invalidate_suggested_questions(&state, &notebook_id);

    // Spawn chunking + embedding + summary in background
    let nb = notebook_id.clone();
    let src = source_id.clone();
    let q = Arc::clone(&queue);
    let handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || {
            let state = handle.state::<AppState>();
            run_ingestion(&nb, &src, &state, &handle, &q);
        })
        .await;
    });

    Ok(source_id)
}

#[tauri::command]
pub async fn delete_source(
    notebook_id: String,
    source_id: String,
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
) -> Result<(), GlossError> {
    let source = state.with_notebook_db(&notebook_id, |db| db.get_source(&source_id))?;
    let old_embedding_ids = state.with_notebook_db(&notebook_id, |db| {
        db.get_embedding_ids_for_source(&source_id)
    })?;
    let notebook_dir = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let notebook = app_db.get_notebook(&notebook_id)?;
        PathBuf::from(notebook.directory)
    };

    let cancelled = jobs::cancel_jobs_matching(&queue, |job, _status| {
        job.notebook_id() == notebook_id && job.source_id() == source_id
    });
    if cancelled > 0 {
        tracing::info!(
            notebook_id,
            source_id,
            cancelled,
            "Cancelled queued background jobs for deleted source"
        );
    }

    if !old_embedding_ids.is_empty() {
        let removed_count = {
            let mut indices = state
                .hnsw_indices
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            if let Some(index) = indices.get_mut(&notebook_id) {
                let mut removed = 0usize;
                for embedding_id in &old_embedding_ids {
                    match index.remove(*embedding_id) {
                        Ok(()) => removed += 1,
                        Err(e) => tracing::warn!(
                            notebook_id,
                            source_id,
                            embedding_id,
                            error = %e,
                            "Failed to remove source vector from HNSW index"
                        ),
                    }
                }
                removed
            } else {
                0
            }
        };

        if removed_count > 0 {
            if let Err(e) = state.save_hnsw_index(&notebook_id) {
                tracing::warn!(
                    notebook_id,
                    source_id,
                    error = %e,
                    "Failed to persist HNSW index after source deletion"
                );
            }
        }
    }

    state.with_notebook_db(&notebook_id, |db| db.delete_source(&source_id))?;
    sync_notebook_source_count(&state, &notebook_id)?;
    invalidate_suggested_questions(&state, &notebook_id);

    if let Some(file_path) = source.file_path.as_deref() {
        let source_file = notebook_dir.join("sources").join(file_path);
        if source_file.exists() {
            if let Err(e) = std::fs::remove_file(&source_file) {
                tracing::warn!(
                    notebook_id,
                    source_id,
                    path = %source_file.display(),
                    error = %e,
                    "Failed to remove deleted source file from disk"
                );
            }
        }
    }

    Ok(())
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
    invalidate_suggested_questions(&state, &notebook_id);
    // Remove old vectors from HNSW index before deleting DB rows
    let old_embedding_ids: Vec<u64> = state.with_notebook_db(&notebook_id, |db| {
        db.get_embedding_ids_for_source(&source_id)
    })?;

    if !old_embedding_ids.is_empty() {
        let removed_count = {
            let mut indices = state
                .hnsw_indices
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            if let Some(index) = indices.get_mut(&notebook_id) {
                let mut removed = 0usize;
                for eid in &old_embedding_ids {
                    match index.remove(*eid) {
                        Ok(()) => removed += 1,
                        Err(e) => tracing::warn!(
                            notebook_id,
                            source_id,
                            embedding_id = eid,
                            error = %e,
                            "Failed to remove old HNSW vector during retry"
                        ),
                    }
                }
                removed
            } else {
                0
            }
        };

        tracing::debug!(count = removed_count, source_id, "Removed old HNSW vectors");
        if removed_count > 0 {
            if let Err(e) = state.save_hnsw_index(&notebook_id) {
                tracing::warn!(
                    notebook_id,
                    source_id,
                    error = %e,
                    "Failed to persist HNSW index after retry cleanup"
                );
            }
        }
    }

    // Reset status and delete old chunks
    let source_type = state.with_notebook_db(&notebook_id, |db| {
        db.update_source_status(&source_id, "pending", None)?;
        db.delete_chunks_for_source(&source_id)?;
        let source = db.get_source(&source_id)?;
        Ok(source.source_type)
    })?;

    // Route based on source type
    match source_type.as_str() {
        "image" => {
            match queue_describe_image_job(&queue, &state, &notebook_id, &source_id, Path::new(""))
            {
                Ok(()) => emit_status(&app_handle, &notebook_id, &source_id, "pending", None),
                Err(msg) => emit_status(&app_handle, &notebook_id, &source_id, "error", Some(&msg)),
            }
        }
        "video" => match queue_describe_video_job(&queue, &state, &notebook_id, &source_id) {
            Ok(()) => emit_status(&app_handle, &notebook_id, &source_id, "pending", None),
            Err(msg) => emit_status(&app_handle, &notebook_id, &source_id, "error", Some(&msg)),
        },
        _ => {
            // Spawn in background so the IPC thread isn't blocked
            let nb = notebook_id.clone();
            let src = source_id.clone();
            let q = Arc::clone(&queue);
            let handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                let _ = tokio::task::spawn_blocking(move || {
                    let state = handle.state::<AppState>();
                    run_ingestion(&nb, &src, &state, &handle, &q);
                })
                .await;
            });
        }
    }
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
) -> Result<QueueSummariesResponse, GlossError> {
    Ok(auto_queue_notebook_summaries(&state, &queue, &notebook_id))
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
) -> QueueSummariesResponse {
    let epoch = queue_epoch_for_notebook(state, notebook_id);
    if jobs::has_jobs_for_notebook_epoch(queue, notebook_id, epoch) {
        tracing::debug!(
            notebook_id,
            epoch,
            "Skipping auto-queue: notebook already has pending or processing jobs"
        );
        return QueueSummariesResponse {
            queued: 0,
            diagnostics: Vec::new(),
        };
    }

    let sources = match state.with_notebook_db(notebook_id, |db| db.list_sources()) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(notebook_id, error = %e, "Failed to list sources for auto-queue");
            return QueueSummariesResponse {
                queued: 0,
                diagnostics: vec![e.to_string()],
            };
        }
    };

    let mut queued = 0u32;
    let mut diagnostics = Vec::new();
    for source in &sources {
        if source.status == "ready" && source.summary.is_none() {
            match queue_summary_job(queue, state, notebook_id, &source.id, &source.title) {
                Ok(true) => queued += 1,
                Ok(false) => {}
                Err(diagnostic) => push_diagnostic(&mut diagnostics, diagnostic),
            }
        }
    }

    if queued > 0 {
        tracing::info!(notebook_id, queued, "Auto-queued missing summary jobs");
    }
    QueueSummariesResponse {
        queued,
        diagnostics,
    }
}

#[derive(Debug, Serialize)]
pub struct QueueStatusResponse {
    pub paused: bool,
    pub mode: String,
    pub pending: u32,
    pub processing: u32,
    pub completed: u32,
    pub failed: u32,
    pub summary_backend: BackgroundBackendStatus,
    pub vision_backend: BackgroundBackendStatus,
}

/// Pause background summary generation.
#[tauri::command]
pub async fn pause_summaries(
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
) -> Result<(), GlossError> {
    state.summary_paused.store(true, Ordering::SeqCst);
    persist_summary_mode(&state, SUMMARY_MODE_MANUAL)?;
    let cancelled = jobs::cancel_jobs_matching(&queue, |_job, status| status == "processing");
    tracing::info!("Summary generation paused by user");
    if cancelled > 0 {
        tracing::info!(cancelled, "Cancelled in-flight background jobs for pause");
    }
    Ok(())
}

/// Resume background summary generation.
#[tauri::command]
pub async fn resume_summaries(
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
) -> Result<(), GlossError> {
    state.summary_paused.store(false, Ordering::SeqCst);
    persist_summary_mode(&state, SUMMARY_MODE_AUTO)?;
    let cancelled = jobs::cancel_jobs_not_matching_active_notebook(
        &queue,
        state.get_active_notebook_id().as_deref(),
        state.get_active_epoch(),
    );
    tracing::info!("Summary generation resumed by user");
    if cancelled > 0 {
        tracing::info!(cancelled, "Cancelled stale jobs on summary resume");
    }
    Ok(())
}

/// Get the current queue status (paused state + job counts).
#[tauri::command]
pub async fn get_queue_status(
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
) -> Result<QueueStatusResponse, GlossError> {
    let paused = state.summary_paused.load(Ordering::SeqCst);
    let stats = queue
        .count_by_status()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    Ok(QueueStatusResponse {
        paused,
        mode: if paused {
            SUMMARY_MODE_MANUAL.to_string()
        } else {
            SUMMARY_MODE_AUTO.to_string()
        },
        pending: stats.pending,
        processing: stats.processing,
        completed: stats.completed,
        failed: stats.failed,
        summary_backend: background_backend_status(&state, BackgroundJobKind::Summary),
        vision_backend: background_backend_status(&state, BackgroundJobKind::Vision),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::app_db::AppDb;
    use crate::db::notebook_db::NotebookDb;
    use crate::providers::ModelRegistry;
    use crate::secrets::SecretStore;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
    use std::sync::Mutex;
    use std::time::Duration;
    use tauri_queue::{QueueConfig, QueueManager};
    use tempfile::TempDir;

    fn build_state() -> (TempDir, AppState, String) {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        std::fs::create_dir_all(data_dir.join("notebooks")).unwrap();

        let app_db = AppDb::open(&data_dir.join("gloss.db")).unwrap();
        let secret_store = SecretStore::new(&data_dir).unwrap();
        let model_registry = ModelRegistry::new(&app_db, &secret_store).unwrap();

        let notebook_id = "nb1".to_string();
        let notebook_dir = data_dir.join("notebooks").join(&notebook_id);
        std::fs::create_dir_all(notebook_dir.join("sources")).unwrap();
        std::fs::create_dir_all(notebook_dir.join("embeddings")).unwrap();
        std::fs::create_dir_all(notebook_dir.join("audio")).unwrap();
        std::fs::create_dir_all(notebook_dir.join("exports")).unwrap();

        app_db
            .create_notebook(&notebook_id, "Notebook", &notebook_dir.to_string_lossy())
            .unwrap();
        NotebookDb::open(&notebook_dir.join("notebook.db")).unwrap();

        let state = AppState {
            app_db: Mutex::new(app_db),
            secret_store,
            notebook_dbs: Mutex::new(HashMap::new()),
            model_registry: Mutex::new(model_registry),
            data_dir,
            embedder: Mutex::new(None),
            hnsw_indices: Mutex::new(HashMap::new()),
            summary_paused: AtomicBool::new(true),
            ingestion_active: AtomicU32::new(0),
            llm_gate: tokio::sync::Semaphore::new(1),
            gpu_gate: tokio::sync::Semaphore::new(1),
            active_notebook_id: Mutex::new(Some(notebook_id.clone())),
            active_epoch: AtomicU64::new(1),
            chat_grace_until: Mutex::new(0),
            last_user_activity: Mutex::new(0),
        };

        (dir, state, notebook_id)
    }

    fn build_queue(dir: &TempDir) -> Arc<QueueManager> {
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

    fn ready_source(id: &str) -> Source {
        Source {
            id: id.to_string(),
            source_type: "text".to_string(),
            title: format!("Source {id}"),
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
        }
    }

    #[test]
    fn summary_config_rejects_non_ollama_default_model() {
        let (_dir, state, _notebook_id) = build_state();
        let app_db = state.app_db.lock().unwrap();
        app_db.set_setting("default_model", "gpt-4o-mini").unwrap();
        app_db.set_setting("default_provider", "openai").unwrap();
        app_db
            .update_provider("openai", true, Some("https://api.openai.com/v1"), None)
            .unwrap();
        app_db
            .replace_models(
                "openai",
                &[ModelRecord {
                    id: "gpt-4o-mini".to_string(),
                    provider_id: "openai".to_string(),
                    display_name: "GPT-4o mini".to_string(),
                    parameter_size: None,
                    context_window: None,
                    capabilities: Some("chat".to_string()),
                }],
            )
            .unwrap();

        let error =
            resolve_background_job_config_from_db(&app_db, BackgroundJobKind::Summary).unwrap_err();
        assert!(error.contains("requires an Ollama model"));
    }

    #[test]
    fn vision_config_rejects_non_vision_model() {
        let (_dir, state, _notebook_id) = build_state();
        let app_db = state.app_db.lock().unwrap();
        app_db.set_setting("default_model", "llama3:8b").unwrap();
        app_db.set_setting("default_provider", "ollama").unwrap();
        app_db
            .replace_models(
                "ollama",
                &[ModelRecord {
                    id: "llama3:8b".to_string(),
                    provider_id: "ollama".to_string(),
                    display_name: "Llama 3 8B".to_string(),
                    parameter_size: None,
                    context_window: None,
                    capabilities: Some("chat".to_string()),
                }],
            )
            .unwrap();

        let error =
            resolve_background_job_config_from_db(&app_db, BackgroundJobKind::Vision).unwrap_err();
        assert!(error.contains("vision-capable"));
    }

    #[test]
    fn auto_queue_reports_zero_when_background_config_is_invalid() {
        let (dir, state, notebook_id) = build_state();
        state
            .with_notebook_db(&notebook_id, |db| db.insert_source(&ready_source("s1")))
            .unwrap();

        {
            let app_db = state.app_db.lock().unwrap();
            app_db.set_setting("default_model", "gpt-4o-mini").unwrap();
            app_db.set_setting("default_provider", "openai").unwrap();
            app_db
                .update_provider("openai", true, Some("https://api.openai.com/v1"), None)
                .unwrap();
            app_db
                .replace_models(
                    "openai",
                    &[ModelRecord {
                        id: "gpt-4o-mini".to_string(),
                        provider_id: "openai".to_string(),
                        display_name: "GPT-4o mini".to_string(),
                        parameter_size: None,
                        context_window: None,
                        capabilities: Some("chat".to_string()),
                    }],
                )
                .unwrap();
        }

        let queue = build_queue(&dir);
        let result = auto_queue_notebook_summaries(&state, &queue, &notebook_id);
        let stats = queue.count_by_status().unwrap();

        assert_eq!(result.queued, 0);
        assert!(!result.diagnostics.is_empty());
        assert_eq!(stats.pending, 0);
        assert_eq!(stats.processing, 0);
    }
}
