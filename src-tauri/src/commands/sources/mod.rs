use crate::db::app_db::ModelRecord;
use crate::db::notebook_db::{Chunk, NotebookStats, SemanticMemoryProjectionSummary, Source};
use crate::error::GlossError;
use crate::features;
use crate::ingestion::chunk::chunk_text_with_title;
use crate::ingestion::extract::extract_text;
use crate::jobs::{self, GlossJob};
use crate::memory::backend::MemorySearchBackend;
use crate::memory::gloss_local::GlossLocalMemoryBackend;
#[cfg(feature = "semantic-memory-backend")]
use crate::memory::semantic_memory_adapter;
use crate::memory::{
    compare_memory_backends_for_notebook, MemoryBackendComparison, MemoryBackendStatus,
    MemorySearchRequest, RetrievalCoverage, SemanticMemoryLinkStatus, MEMORY_BACKEND_GLOSS_LOCAL,
};
use crate::retrieval::source_scope::SourceScope;
use crate::state::{ActiveCounterGuard, AppState, SUMMARY_MODE_AUTO, SUMMARY_MODE_MANUAL};
use rusqlite::OptionalExtension;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;
use tauri::{Emitter, Manager, State};
use tauri_queue::{QueueJob, QueueManager, QueuePriority};

// Profile status is owned by settings.rs through get_semantic_memory_profile_status.

#[derive(Debug, Serialize)]
pub struct SourceContent {
    pub content_text: Option<String>,
    pub word_count: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct SemanticMemorySourceProjectionError {
    pub source_id: String,
    pub title: String,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct SemanticMemoryBackfillReceipt {
    pub notebook_id: String,
    pub receipt_id: String,
    pub total_sources: usize,
    pub chunk_bearing_sources: usize,
    pub projected_sources: usize,
    pub skipped_no_chunks: usize,
    pub failed_sources: usize,
    pub stale_sources: usize,
    pub total_chunks: usize,
    pub projected_chunks: usize,
    pub errors: Vec<SemanticMemorySourceProjectionError>,
    pub vector_artifact_receipt: Option<serde_json::Value>,
    pub projection_summary: SemanticMemoryProjectionSummary,
}

#[derive(Debug, Clone, Serialize)]
pub struct VectorArtifactStatus {
    pub compiled_turbo_quant: bool,
    pub runtime_turbo_quant_enabled: bool,
    pub candidate_backend: Option<String>,
    pub artifact_generation_id: Option<String>,
    pub vector_artifact_manifest_digest: Option<String>,
    pub vector_artifact_missing_count: usize,
    pub vector_artifact_stale_count: usize,
    pub exact_rerank: bool,
    pub exact_rerank_count: usize,
    pub last_receipt_id: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RetrievalDiagnostics {
    pub query: String,
    pub scope_kind: String,
    pub scoped_sources: usize,
    pub scoped_chunks: usize,
    pub fts_indexed_chunks: usize,
    pub bm25_hit_count: usize,
    pub semantic_links_total: usize,
    pub semantic_links_healthy: usize,
    pub semantic_links_missing: usize,
    pub semantic_links_degraded: usize,
    pub semantic_search_attempted: bool,
    pub semantic_candidate_count: usize,
    pub candidate_backend: Option<String>,
    pub fallback_allowed: bool,
    pub fallback_used: bool,
    pub fallback_reason: Option<String>,
    pub retrieval_mode: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetrievalProbeReceipt {
    pub receipt_id: String,
    pub notebook_id: String,
    pub query_digest: String,
    pub source_scope_kind: String,
    pub scoped_sources: usize,
    pub scoped_chunks: usize,
    pub backend_requested: String,
    pub backend_used: String,
    pub bm25_candidates: usize,
    pub vector_candidates: usize,
    pub tq_candidates: usize,
    pub candidate_backend: Option<String>,
    pub artifact_generation_id: Option<String>,
    pub vector_artifact_manifest_digest: Option<String>,
    pub exact_rerank: bool,
    pub exact_rerank_count: usize,
    pub fallback_used: bool,
    pub fallback_reason: Option<String>,
    pub degradation_markers: Vec<String>,
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
    // Signal that ingestion is active so the summary loop yields. The guard
    // finalizes the counter on early returns or panic unwind.
    let _ingestion_guard = ActiveCounterGuard::new(&state.ingestion_active, "ingestion_active");

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
        let db_chunks = chunks
            .iter()
            .map(|chunk_data| Chunk {
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
            })
            .collect::<Vec<_>>();
        state.with_notebook_db(notebook_id, |db| db.insert_chunks(&db_chunks))?;

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

        if let Err(error) =
            maybe_auto_project_semantic_memory(notebook_id, source_id, state, app_handle)
        {
            tracing::warn!(
                notebook_id,
                source_id,
                error = %error,
                "semantic-memory projection failed after source import"
            );
            let message = error.to_string();
            emit_status(
                app_handle,
                notebook_id,
                source_id,
                "semantic_memory_error",
                Some(&message),
            );
        }

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

#[cfg(feature = "semantic-memory-backend")]
fn semantic_memory_runtime_config_from_state(
    state: &AppState,
) -> Result<semantic_memory_adapter::SemanticMemoryRuntimeConfig, GlossError> {
    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    features::require_semantic_memory_preview_enabled(&app_db)?;
    semantic_memory_runtime_config_from_app_db(&app_db)
}

#[cfg(feature = "semantic-memory-backend")]
fn semantic_memory_runtime_config_from_app_db(
    app_db: &crate::db::app_db::AppDb,
) -> Result<semantic_memory_adapter::SemanticMemoryRuntimeConfig, GlossError> {
    let config = semantic_memory_adapter::runtime_config_from_settings(
        app_db.get_setting("semantic_memory_embedding_url")?,
        app_db.get_setting("semantic_memory_embedding_model")?,
        app_db.get_setting("semantic_memory_embedding_timeout_secs")?,
        features::turbo_quant_active(app_db)?,
        crate::commands::chat::setting_is_enabled(
            app_db.get_setting(features::SEMANTIC_MEMORY_TURBO_QUANT_REQUIRE_FRESH_ARTIFACTS)?,
        ),
    );
    semantic_memory_adapter::validate_embedding_model_role(&config)?;
    Ok(config)
}

#[cfg(feature = "semantic-memory-backend")]
fn maybe_auto_project_semantic_memory(
    notebook_id: &str,
    source_id: &str,
    state: &AppState,
    app_handle: &tauri::AppHandle,
) -> Result<(), GlossError> {
    let runtime_config = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        if !crate::commands::chat::setting_is_enabled(
            app_db.get_setting(features::SEMANTIC_MEMORY_AUTO_PROJECT)?,
        ) || !features::semantic_memory_preview_active(&app_db)?
        {
            return Ok(());
        }
        semantic_memory_runtime_config_from_app_db(&app_db)?
    };

    emit_status(
        app_handle,
        notebook_id,
        source_id,
        "semantic_memory_projecting",
        None,
    );
    let receipt_id = uuid::Uuid::new_v4().to_string();
    let db_path = state.notebook_db_path(notebook_id)?;
    match tauri::async_runtime::block_on(semantic_memory_adapter::reindex_source(
        &state.data_dir,
        notebook_id,
        &db_path,
        source_id,
        Some(receipt_id),
        Some(runtime_config),
    )) {
        Ok(receipt) => {
            let message = serde_json::to_string(&receipt).ok();
            emit_status(
                app_handle,
                notebook_id,
                source_id,
                "semantic_memory_synced",
                message.as_deref(),
            );
            Ok(())
        }
        Err(error) => {
            let message = error.to_string();
            emit_status(
                app_handle,
                notebook_id,
                source_id,
                "semantic_memory_error",
                Some(&message),
            );
            Err(error)
        }
    }
}

#[cfg(not(feature = "semantic-memory-backend"))]
fn maybe_auto_project_semantic_memory(
    _notebook_id: &str,
    _source_id: &str,
    _state: &AppState,
    _app_handle: &tauri::AppHandle,
) -> Result<(), GlossError> {
    Ok(())
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

fn vector_artifact_receipt_fields(
    receipt: &serde_json::Value,
) -> (Option<String>, Option<String>, Option<String>) {
    let receipt_id = receipt
        .get("build_receipt_id")
        .or_else(|| receipt.get("receipt_id"))
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let generation_id = receipt
        .get("generation_id")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let manifest_digest = receipt
        .get("artifact_manifest_digest")
        .or_else(|| receipt.get("vector_artifact_manifest_digest"))
        .and_then(|value| value.as_str())
        .map(str::to_string);
    (receipt_id, generation_id, manifest_digest)
}

fn record_vector_artifact_receipt(
    state: &AppState,
    notebook_id: &str,
    receipt: &serde_json::Value,
) -> Result<(), GlossError> {
    let (receipt_id, generation_id, manifest_digest) = vector_artifact_receipt_fields(receipt);
    let receipt_id = receipt_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let status = receipt
        .get("status")
        .and_then(|value| value.as_str())
        .unwrap_or("ok");
    let raw_receipt_json =
        serde_json::to_string(receipt).map_err(|error| GlossError::Other(error.to_string()))?;
    state.with_notebook_db(notebook_id, |db| {
        db.conn.execute(
            "INSERT OR REPLACE INTO semantic_memory_vector_artifact_receipts
             (receipt_id, notebook_id, generation_id, artifact_manifest_digest, raw_receipt_json, status, recorded_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))",
            rusqlite::params![
                receipt_id,
                notebook_id,
                generation_id.as_deref(),
                manifest_digest.as_deref(),
                raw_receipt_json,
                status
            ],
        )?;
        db.update_semantic_memory_projection_artifact(
            notebook_id,
            Some(&receipt_id),
            generation_id.as_deref(),
            manifest_digest.as_deref(),
            None,
        )
    })
}

fn query_digest(query: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(query.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn receipt_usize(receipt: &serde_json::Value, key: &str) -> usize {
    receipt
        .get(key)
        .and_then(|value| value.as_u64())
        .unwrap_or(0) as usize
}

fn receipt_string(receipt: &serde_json::Value, key: &str) -> Option<String> {
    receipt
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn record_retrieval_probe_receipt(
    state: &AppState,
    receipt: &RetrievalProbeReceipt,
    raw_receipt: &serde_json::Value,
) -> Result<(), GlossError> {
    let degradation_markers = serde_json::to_string(&receipt.degradation_markers)
        .map_err(|error| GlossError::Other(error.to_string()))?;
    let raw_receipt_json =
        serde_json::to_string(raw_receipt).map_err(|error| GlossError::Other(error.to_string()))?;
    state.with_notebook_db(&receipt.notebook_id, |db| {
        db.conn.execute(
            "INSERT OR REPLACE INTO semantic_memory_retrieval_probe_receipts
             (receipt_id, notebook_id, query_digest, source_scope_kind, scoped_sources, scoped_chunks,
              backend_requested, backend_used, bm25_candidates, vector_candidates, tq_candidates,
              candidate_backend, artifact_generation_id, vector_artifact_manifest_digest,
              exact_rerank, exact_rerank_count, fallback_used, fallback_reason,
              degradation_markers, raw_receipt_json, recorded_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, datetime('now'))",
            rusqlite::params![
                receipt.receipt_id,
                receipt.notebook_id,
                receipt.query_digest,
                receipt.source_scope_kind,
                receipt.scoped_sources as i64,
                receipt.scoped_chunks as i64,
                receipt.backend_requested,
                receipt.backend_used,
                receipt.bm25_candidates as i64,
                receipt.vector_candidates as i64,
                receipt.tq_candidates as i64,
                receipt.candidate_backend.as_deref(),
                receipt.artifact_generation_id.as_deref(),
                receipt.vector_artifact_manifest_digest.as_deref(),
                if receipt.exact_rerank { 1_i64 } else { 0_i64 },
                receipt.exact_rerank_count as i64,
                if receipt.fallback_used { 1_i64 } else { 0_i64 },
                receipt.fallback_reason.as_deref(),
                degradation_markers,
                raw_receipt_json,
            ],
        )?;
        Ok(())
    })
}

type LatestRetrievalProbeTurboProof = (
    String,
    bool,
    usize,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn latest_retrieval_probe_turbo_proof(
    state: &AppState,
    notebook_id: &str,
) -> Result<Option<LatestRetrievalProbeTurboProof>, GlossError> {
    state.with_notebook_db(notebook_id, |db| {
        db.conn
            .query_row(
                "SELECT receipt_id, exact_rerank, exact_rerank_count, candidate_backend,
                        artifact_generation_id, vector_artifact_manifest_digest
                 FROM semantic_memory_retrieval_probe_receipts
                 WHERE notebook_id = ?1
                 ORDER BY recorded_at DESC LIMIT 1",
                [notebook_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)? == 1,
                        row.get::<_, i64>(2)?.max(0) as usize,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(GlossError::Database)
    })
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
    let model_record = models
        .iter()
        .find(|model| model.id == selected_model && model.available && !model.stale)
        .ok_or_else(|| {
            format!(
                "Background {} model '{}' is not available in the model registry. Refresh models or choose a compatible Ollama model.",
                kind.label(),
                selected_model
            )
        })?;
    let provider_id = model_record.provider_id.clone();

    if provider_id != "ollama" {
        return Err(format!(
            "Background {} requires an Ollama model, but '{}' uses provider '{}'.",
            kind.label(),
            selected_model,
            provider_id
        ));
    }

    if matches!(kind, BackgroundJobKind::Vision) && !has_vision_capability(model_record) {
        return Err(format!(
            "Background vision requires a vision-capable Ollama model, but '{}' is not marked as vision-capable.",
            selected_model
        ));
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

/// Finalize a described media source by marking it ready and queueing summary
/// generation. This does not create semantic embeddings.
pub(crate) fn finalize_described_source(
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

    let finalize_error_msg = if let Err(ref e) = result {
        tracing::warn!(source_id, error = %e, "Finalization failed for described source");
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
        finalize_error_msg.as_deref(),
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

fn emit_folder_scan_event(
    app_handle: &tauri::AppHandle,
    notebook_id: &str,
    status: &str,
    path: &str,
    count: usize,
    message: Option<&str>,
) {
    let _ = app_handle.emit(
        "sources:folder_scan",
        serde_json::json!({
            "notebook_id": notebook_id,
            "status": status,
            "path": path,
            "count": count,
            "message": message,
        }),
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_import_batch_receipt(
    app_handle: &tauri::AppHandle,
    notebook_id: &str,
    import_batch_id: &str,
    notebook_epoch: u64,
    status: &str,
    found: usize,
    created: usize,
    ingested_ready: usize,
    failed: usize,
    skipped_duplicate: usize,
    skipped_unsupported: usize,
    cancelled_superseded: usize,
    message: Option<&str>,
) {
    let _ = app_handle.emit(
        "sources:batch_ingestion_complete",
        serde_json::json!({
            "notebook_id": notebook_id,
            "import_batch_id": import_batch_id,
            "notebook_epoch": notebook_epoch,
            "status": status,
            "found": found,
            "created": created,
            "ingested_ready": ingested_ready,
            "failed": failed,
            "skipped_duplicate": skipped_duplicate,
            "skipped_unsupported": skipped_unsupported,
            "cancelled_superseded": cancelled_superseded,
            "count": ingested_ready,
            "message": message,
        }),
    );
}

fn validate_import_batch_notebook(
    app_handle: &tauri::AppHandle,
    notebook_id: &str,
    notebook_epoch: u64,
) -> Result<(), String> {
    let state = app_handle.state::<AppState>();
    {
        let app_db = state.app_db.lock().map_err(|e| e.to_string())?;
        app_db
            .get_notebook(notebook_id)
            .map_err(|e| e.to_string())?;
    }
    if !state.is_active_notebook_epoch(notebook_id, notebook_epoch) {
        return Err("cancelled_superseded".to_string());
    }
    Ok(())
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

fn validate_import_size(source_path: &Path) -> Result<(), GlossError> {
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

    validate_import_size(&source_path)?;

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

#[tauri::command]
pub async fn add_source_files(
    notebook_id: String,
    paths: Vec<String>,
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
    app_handle: tauri::AppHandle,
) -> Result<Vec<String>, GlossError> {
    let mut created = Vec::new();
    for path in paths {
        let source_path = PathBuf::from(&path);
        validate_import_size(&source_path)?;
        let (source_id, source_type) =
            create_file_source(&notebook_id, &source_path, None, &state)?;

        match source_type.as_str() {
            "image" => {
                match queue_describe_image_job(
                    &queue,
                    &state,
                    &notebook_id,
                    &source_id,
                    &source_path,
                ) {
                    Ok(()) => emit_status(&app_handle, &notebook_id, &source_id, "pending", None),
                    Err(msg) => {
                        emit_status(&app_handle, &notebook_id, &source_id, "error", Some(&msg))
                    }
                }
            }
            "video" => match queue_describe_video_job(&queue, &state, &notebook_id, &source_id) {
                Ok(()) => emit_status(&app_handle, &notebook_id, &source_id, "pending", None),
                Err(msg) => emit_status(&app_handle, &notebook_id, &source_id, "error", Some(&msg)),
            },
            _ => {
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
        created.push(source_id);
    }

    if !created.is_empty() {
        sync_notebook_source_count(&state, &notebook_id)?;
        invalidate_suggested_questions(&state, &notebook_id);
        let _ = app_handle.emit(
            "sources:batch_created",
            serde_json::json!({
                "notebook_id": &notebook_id,
                "count": created.len(),
            }),
        );
    }

    Ok(created)
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

    let mut entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries.filter_map(Result::ok).collect::<Vec<_>>(),
        Err(e) => {
            tracing::warn!(dir = %dir.display(), error = %e, "Cannot read directory, skipping");
            return;
        }
    };
    entries.sort_by_key(|a| a.path());

    for entry in entries {
        if out.len() >= MAX_WALK_FILES {
            tracing::warn!("File limit ({}) reached during folder walk", MAX_WALK_FILES);
            return;
        }

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
    {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let _notebook = app_db.get_notebook(&notebook_id)?;
    }
    let notebook_epoch = state.get_active_epoch();
    if !state.is_active_notebook_epoch(&notebook_id, notebook_epoch) {
        return Err(GlossError::Ingestion {
            source_id: String::new(),
            message: "cancelled_superseded".to_string(),
        });
    }
    invalidate_suggested_questions(&state, &notebook_id);
    let folder = PathBuf::from(&path);
    if !folder.is_dir() {
        emit_folder_scan_event(
            &app_handle,
            &notebook_id,
            "scan_failed",
            &path,
            0,
            Some("Not a directory"),
        );
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
    let import_batch_id = uuid::Uuid::new_v4().to_string();
    tauri::async_runtime::spawn(async move {
        emit_folder_scan_event(&handle, &nb_id, "scan_started", &import_path, 0, None);
        let batch_ingestion_guard = {
            let state = handle.state::<AppState>();
            ActiveCounterGuard::new(&state.ingestion_active, "batch_ingestion_active")
        };
        if let Err(reason) = validate_import_batch_notebook(&handle, &nb_id, notebook_epoch) {
            emit_import_batch_receipt(
                &handle,
                &nb_id,
                &import_batch_id,
                notebook_epoch,
                "cancelled_superseded",
                0,
                0,
                0,
                0,
                0,
                0,
                1,
                Some(&reason),
            );
            emit_folder_scan_event(
                &handle,
                &nb_id,
                "cancelled_superseded",
                &import_path,
                0,
                Some(&reason),
            );
            return;
        }

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
                emit_folder_scan_event(
                    &handle,
                    &nb_id,
                    "scan_failed",
                    &import_path,
                    0,
                    Some(&e.to_string()),
                );
                emit_import_batch_receipt(
                    &handle,
                    &nb_id,
                    &import_batch_id,
                    notebook_epoch,
                    "failed",
                    0,
                    0,
                    0,
                    1,
                    0,
                    0,
                    0,
                    Some(&e.to_string()),
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
            emit_folder_scan_event(
                &handle,
                &nb_id,
                "scan_truncated",
                &import_path,
                files.len(),
                Some("Folder scan reached the supported file cap"),
            );
        }

        let file_count = files.len();
        let found = file_count;
        tracing::info!(
            folder = %import_path,
            files_found = file_count,
            "Directory walk complete"
        );
        if let Err(reason) = validate_import_batch_notebook(&handle, &nb_id, notebook_epoch) {
            emit_import_batch_receipt(
                &handle,
                &nb_id,
                &import_batch_id,
                notebook_epoch,
                "cancelled_superseded",
                found,
                0,
                0,
                0,
                0,
                0,
                found,
                Some(&reason),
            );
            emit_folder_scan_event(
                &handle,
                &nb_id,
                "cancelled_superseded",
                &import_path,
                found,
                Some(&reason),
            );
            return;
        }

        if file_count == 0 {
            emit_folder_scan_event(
                &handle,
                &nb_id,
                "scan_empty",
                &import_path,
                0,
                Some("No supported files found"),
            );
            emit_import_batch_receipt(
                &handle,
                &nb_id,
                &import_batch_id,
                notebook_epoch,
                "empty",
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                Some("No supported files found"),
            );
            return;
        }

        // Phase 1: Create source records (blocking I/O — file reads + copies)
        let mut sources: Vec<(String, String)> = Vec::new();
        let mut failed = 0usize;
        let mut skipped_duplicate = 0usize;
        for batch in files.chunks(SOURCE_CREATION_BATCH_SIZE) {
            if let Err(reason) = validate_import_batch_notebook(&handle, &nb_id, notebook_epoch) {
                emit_import_batch_receipt(
                    &handle,
                    &nb_id,
                    &import_batch_id,
                    notebook_epoch,
                    "cancelled_superseded",
                    found,
                    sources.len(),
                    0,
                    failed,
                    skipped_duplicate,
                    0,
                    found.saturating_sub(sources.len() + skipped_duplicate + failed),
                    Some(&reason),
                );
                emit_folder_scan_event(
                    &handle,
                    &nb_id,
                    "cancelled_superseded",
                    &import_path,
                    sources.len(),
                    Some(&reason),
                );
                return;
            }
            let batch_files = batch.to_vec();
            let handle_p1 = handle.clone();
            let nb_id_p1 = nb_id.clone();
            let folder_p1 = folder_create.clone();
            let import_batch_id_p1 = import_batch_id.clone();
            let created = tokio::task::spawn_blocking(move || {
                let state = handle_p1.state::<AppState>();
                let mut created: Vec<(String, String)> = Vec::new();
                let mut failed = 0usize;
                let mut skipped_duplicate = 0usize;
                for file_path in batch_files {
                    if !state.is_active_notebook_epoch(&nb_id_p1, notebook_epoch) {
                        tracing::info!(
                            notebook_id = %nb_id_p1,
                            import_batch_id = %import_batch_id_p1,
                            "Folder import cancelled_superseded before source creation"
                        );
                        break;
                    }
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
                            failed += 1;
                            continue;
                        }
                    }

                    match create_file_source(&nb_id_p1, &file_path, Some(&folder_p1), &state) {
                        Ok((source_id, source_type)) => created.push((source_id, source_type)),
                        Err(GlossError::Ingestion { message, .. }) if message == "duplicate" => {
                            skipped_duplicate += 1;
                        }
                        Err(e) => {
                            failed += 1;
                            tracing::warn!(
                                file = %file_path.display(),
                                error = %e,
                                "Skipping file during folder import"
                            );
                        }
                    }
                }
                (created, skipped_duplicate, failed)
            })
            .await
            .unwrap_or_default();

            let (created_batch, skipped_duplicate_batch, failed_batch) = created;
            skipped_duplicate += skipped_duplicate_batch;
            failed += failed_batch;
            if created_batch.is_empty() {
                continue;
            }

            sources.extend(created_batch);
            let _ = handle.emit(
                "sources:batch_created",
                serde_json::json!({
                    "notebook_id": &nb_id,
                    "import_batch_id": &import_batch_id,
                    "notebook_epoch": notebook_epoch,
                    "count": sources.len(),
                    "created": sources.len(),
                    "found": found,
                }),
            );

            tokio::task::yield_now().await;
        }

        let total = sources.len();
        if total == 0 {
            emit_folder_scan_event(
                &handle,
                &nb_id,
                "import_completed",
                &import_path,
                0,
                Some("No new sources created"),
            );
            emit_import_batch_receipt(
                &handle,
                &nb_id,
                &import_batch_id,
                notebook_epoch,
                "completed_empty",
                found,
                0,
                0,
                failed,
                skipped_duplicate,
                0,
                0,
                Some("No new sources created"),
            );
            return;
        }

        // Phase 2: Ingest one at a time on blocking thread pool.
        // spawn_blocking isolates panics from ONNX/usearch C++ code.
        // Images are routed to the vision pipeline (queued jobs) instead.
        for (i, (source_id, source_type)) in sources.into_iter().enumerate() {
            if let Err(reason) = validate_import_batch_notebook(&handle, &nb_id, notebook_epoch) {
                emit_import_batch_receipt(
                    &handle,
                    &nb_id,
                    &import_batch_id,
                    notebook_epoch,
                    "cancelled_superseded",
                    found,
                    total,
                    i,
                    failed,
                    skipped_duplicate,
                    0,
                    total.saturating_sub(i),
                    Some(&reason),
                );
                emit_folder_scan_event(
                    &handle,
                    &nb_id,
                    "cancelled_superseded",
                    &import_path,
                    i,
                    Some(&reason),
                );
                return;
            }
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
                failed += 1;
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

        // Release the batch-level ingestion hold before post-processing.
        drop(batch_ingestion_guard);

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
                "import_batch_id": &import_batch_id,
                "notebook_epoch": notebook_epoch,
                "status": "completed",
                "found": found,
                "created": total,
                "ingested_ready": total.saturating_sub(failed),
                "failed": failed,
                "skipped_duplicate": skipped_duplicate,
                "skipped_unsupported": 0,
                "cancelled_superseded": 0,
                "count": total.saturating_sub(failed),
                "message": null,
            }),
        );
        emit_folder_scan_event(
            &handle,
            &nb_id,
            "import_completed",
            &import_path,
            total,
            None,
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

    state.with_notebook_db(&notebook_id, |db| {
        db.conn.execute(
            "UPDATE semantic_memory_links
             SET sync_status = 'stale',
                 sync_error = 'Gloss source deleted',
                 synced_at = datetime('now')
             WHERE notebook_id = ?1 AND source_id = ?2",
            rusqlite::params![notebook_id, source_id],
        )?;
        db.delete_source(&source_id)
    })?;
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
pub async fn delete_sources(
    notebook_id: String,
    source_ids: Vec<String>,
    state: State<'_, AppState>,
    queue: State<'_, Arc<QueueManager>>,
) -> Result<(), GlossError> {
    let notebook_dir = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let notebook = app_db.get_notebook(&notebook_id)?;
        PathBuf::from(notebook.directory)
    };

    for source_id in &source_ids {
        let source = state.with_notebook_db(&notebook_id, |db| db.get_source(source_id))?;
        let old_embedding_ids = state.with_notebook_db(&notebook_id, |db| {
            db.get_embedding_ids_for_source(source_id)
        })?;

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
            let mut indices = state
                .hnsw_indices
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            if let Some(index) = indices.get_mut(&notebook_id) {
                for embedding_id in &old_embedding_ids {
                    if let Err(e) = index.remove(*embedding_id) {
                        tracing::warn!(
                            notebook_id,
                            source_id,
                            embedding_id,
                            error = %e,
                            "Failed to remove source vector from HNSW index"
                        );
                    }
                }
            }
        }

        state.with_notebook_db(&notebook_id, |db| {
            db.conn.execute(
                "UPDATE semantic_memory_links
                 SET sync_status = 'stale',
                     sync_error = 'Gloss source deleted',
                     synced_at = datetime('now')
                 WHERE notebook_id = ?1 AND source_id = ?2",
                rusqlite::params![notebook_id, source_id],
            )?;
            db.delete_source(source_id)
        })?;

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
    }

    if !source_ids.is_empty() {
        if let Err(e) = state.save_hnsw_index(&notebook_id) {
            tracing::warn!(
                notebook_id,
                error = %e,
                "Failed to persist HNSW index after bulk source deletion"
            );
        }
        sync_notebook_source_count(&state, &notebook_id)?;
        invalidate_suggested_questions(&state, &notebook_id);
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
        db.conn.execute(
            "UPDATE semantic_memory_links
             SET sync_status = 'stale',
                 sync_error = 'Gloss source reingestion requested',
                 synced_at = datetime('now')
             WHERE notebook_id = ?1 AND source_id = ?2",
            rusqlite::params![notebook_id, source_id],
        )?;
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

#[tauri::command]
pub async fn diagnose_retrieval_coverage(
    notebook_id: String,
    source_scope: Option<SourceScope>,
    state: State<'_, AppState>,
) -> Result<RetrievalCoverage, GlossError> {
    state.with_notebook_db(&notebook_id, |db| {
        let sources = db.list_sources()?;
        let resolved = source_scope.unwrap_or(SourceScope::All).resolve(&sources);
        db.retrieval_coverage(&resolved)
    })
}

#[tauri::command]
pub async fn diagnose_retrieval_query(
    notebook_id: String,
    query: String,
    source_scope: SourceScope,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<RetrievalDiagnostics, GlossError> {
    let probe = run_retrieval_probe(
        notebook_id.clone(),
        query.clone(),
        source_scope.clone(),
        limit,
        state.clone(),
    )
    .await?;
    let coverage = state.with_notebook_db(&notebook_id, |db| {
        let sources = db.list_sources()?;
        let resolved = source_scope.resolve(&sources);
        db.retrieval_coverage(&resolved)
    })?;
    let fallback_allowed = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        crate::commands::chat::setting_is_enabled(app_db.get_setting("memory_backend_fallback")?)
    };
    let vector_status =
        semantic_memory_vector_artifact_status(notebook_id.clone(), state.clone()).await?;
    let semantic_links_missing = coverage
        .total_chunks
        .saturating_sub(coverage.semantic_links_healthy);
    let semantic_ready = coverage.total_chunks > 0 && semantic_links_missing == 0;
    let fallback_used = !semantic_ready && fallback_allowed;
    let fallback_reason = if !semantic_ready {
        Some("semantic-memory projection required for selected scope".to_string())
    } else {
        None
    };

    Ok(RetrievalDiagnostics {
        query,
        scope_kind: probe.source_scope_kind,
        scoped_sources: probe.scoped_sources,
        scoped_chunks: coverage.total_chunks,
        fts_indexed_chunks: coverage.fts_indexed_chunks,
        bm25_hit_count: probe.bm25_candidates,
        semantic_links_total: coverage.semantic_links_total,
        semantic_links_healthy: coverage.semantic_links_healthy,
        semantic_links_missing,
        semantic_links_degraded: coverage.semantic_links_degraded,
        semantic_search_attempted: semantic_ready,
        semantic_candidate_count: probe.vector_candidates,
        candidate_backend: vector_status.candidate_backend,
        fallback_allowed,
        fallback_used,
        fallback_reason,
        retrieval_mode: if semantic_ready {
            "semantic_memory".to_string()
        } else if probe.bm25_candidates > 0 {
            "bm25_only".to_string()
        } else {
            "unavailable".to_string()
        },
    })
}

#[tauri::command]
pub async fn run_retrieval_probe(
    notebook_id: String,
    query: String,
    source_scope: SourceScope,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<RetrievalProbeReceipt, GlossError> {
    let comparison = compare_memory_backends(
        notebook_id.clone(),
        query.clone(),
        source_scope.clone(),
        limit,
        state.clone(),
    )
    .await?;
    let (scope_kind, scoped_sources, scoped_chunks) =
        state.with_notebook_db(&notebook_id, |db| {
            let sources = db.list_sources()?;
            let resolved = source_scope.resolve(&sources);
            let coverage = db.retrieval_coverage(&resolved)?;
            Ok((
                format!("{:?}", resolved.kind()).to_ascii_lowercase(),
                resolved.source_count(),
                coverage.total_chunks,
            ))
        })?;
    let semantic = comparison.semantic_memory_result.as_ref();
    let raw_receipt = semantic
        .and_then(|result| result.provenance.get("semantic_memory_receipt"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let candidate_backend = receipt_string(&raw_receipt, "candidate_backend");
    let vector_candidates = semantic.map(|result| result.candidates.len()).unwrap_or(0);
    let tq_candidates = if candidate_backend
        .as_deref()
        .is_some_and(|backend| backend.contains("turbo_quant"))
    {
        vector_candidates
    } else {
        0
    };
    let mut degradation_markers = semantic
        .map(|result| result.degradation_markers.clone())
        .unwrap_or_default();
    degradation_markers.extend(comparison.unmapped_semantic_candidates.clone());
    degradation_markers.sort();
    degradation_markers.dedup();

    let receipt = RetrievalProbeReceipt {
        receipt_id: uuid::Uuid::new_v4().to_string(),
        notebook_id: notebook_id.clone(),
        query_digest: query_digest(&query),
        source_scope_kind: scope_kind,
        scoped_sources,
        scoped_chunks,
        backend_requested: semantic
            .map(|result| result.backend_requested.clone())
            .unwrap_or_else(|| MEMORY_BACKEND_GLOSS_LOCAL.to_string()),
        backend_used: semantic
            .map(|result| result.backend_used.clone())
            .unwrap_or_else(|| MEMORY_BACKEND_GLOSS_LOCAL.to_string()),
        bm25_candidates: comparison.local_backend_result.candidates.len(),
        vector_candidates,
        tq_candidates,
        candidate_backend,
        artifact_generation_id: receipt_string(&raw_receipt, "artifact_generation_id"),
        vector_artifact_manifest_digest: receipt_string(
            &raw_receipt,
            "vector_artifact_manifest_digest",
        ),
        exact_rerank: raw_receipt
            .get("exact_rerank")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        exact_rerank_count: receipt_usize(&raw_receipt, "exact_rerank_count"),
        fallback_used: semantic.map(|result| result.fallback_used).unwrap_or(true),
        fallback_reason: semantic.and_then(|result| result.fallback_reason.clone()),
        degradation_markers,
    };
    record_retrieval_probe_receipt(&state, &receipt, &raw_receipt)?;
    Ok(receipt)
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

    let sources =
        match state.with_notebook_db(notebook_id, |db| db.list_source_headers_needing_summary()) {
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
        match queue_summary_job(queue, state, notebook_id, &source.id, &source.title) {
            Ok(true) => queued += 1,
            Ok(false) => {}
            Err(diagnostic) => push_diagnostic(&mut diagnostics, diagnostic),
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
    pub gate_owners: Vec<crate::state::RuntimeGateOwner>,
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
        gate_owners: state.gate_owners_snapshot(),
        summary_backend: background_backend_status(&state, BackgroundJobKind::Summary),
        vision_backend: background_backend_status(&state, BackgroundJobKind::Vision),
    })
}

fn active_memory_backend(state: &AppState) -> Result<String, GlossError> {
    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    let backend = app_db
        .get_setting("memory_backend")?
        .unwrap_or_else(|| MEMORY_BACKEND_GLOSS_LOCAL.to_string());
    if backend == crate::memory::MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW
        && !features::semantic_memory_preview_active(&app_db)?
    {
        return Ok(MEMORY_BACKEND_GLOSS_LOCAL.to_string());
    }
    Ok(backend)
}

fn link_status_for_notebook(
    state: &AppState,
    notebook_id: &str,
) -> Result<SemanticMemoryLinkStatus, GlossError> {
    state.with_notebook_db(notebook_id, |db| {
        let (total, synced, stale, failed, missing_docs, last_error): (
            i64,
            i64,
            i64,
            i64,
            i64,
            Option<String>,
        ) = db.conn.query_row(
            "SELECT
                COUNT(*),
                SUM(CASE WHEN sync_status = 'synced' THEN 1 ELSE 0 END),
                SUM(CASE WHEN sync_status = 'stale' THEN 1 ELSE 0 END),
                SUM(CASE WHEN sync_status = 'failed' THEN 1 ELSE 0 END),
                SUM(CASE WHEN sm_document_id IS NULL THEN 1 ELSE 0 END),
                (SELECT sync_error FROM semantic_memory_links
                 WHERE sync_error IS NOT NULL AND sync_error != ''
                 ORDER BY synced_at DESC LIMIT 1)
             FROM semantic_memory_links",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )?;

        let degraded_links =
            stale.max(0) as usize + failed.max(0) as usize + missing_docs.max(0) as usize;
        let mut reason_codes = Vec::new();
        if total == 0 {
            reason_codes.push("semantic_memory_links_missing".to_string());
        }
        if stale > 0 || failed > 0 || missing_docs > 0 {
            reason_codes.push("semantic_memory_links_degraded".to_string());
        }

        Ok(SemanticMemoryLinkStatus {
            notebook_id: notebook_id.to_string(),
            total_links: total.max(0) as usize,
            synced_links: synced.max(0) as usize,
            stale_links: stale.max(0) as usize,
            failed_links: failed.max(0) as usize,
            missing_document_links: missing_docs.max(0) as usize,
            degraded_links,
            reason_codes,
            last_sync_error: last_error,
        })
    })
}

#[tauri::command]
pub async fn memory_backend_status(
    notebook_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<MemoryBackendStatus, GlossError> {
    let active_backend = active_memory_backend(&state)?;
    let (requested_backend, semantic_memory_feature_enabled, semantic_memory_available) = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        (
            app_db
                .get_setting("memory_backend")?
                .unwrap_or_else(|| MEMORY_BACKEND_GLOSS_LOCAL.to_string()),
            cfg!(feature = "semantic-memory-backend"),
            features::semantic_memory_preview_active(&app_db)?,
        )
    };
    let link_status = notebook_id
        .as_deref()
        .map(|id| link_status_for_notebook(&state, id))
        .transpose()?;

    let index_sync_status = link_status
        .as_ref()
        .map(|status| {
            if status.failed_links > 0 {
                "failed"
            } else if status.stale_links > 0 || status.missing_document_links > 0 {
                "degraded"
            } else if status.synced_links > 0 {
                "synced"
            } else {
                "empty"
            }
        })
        .unwrap_or("unknown")
        .to_string();
    let mut degradation_markers = Vec::new();
    let mut fallback_reason = None;
    if requested_backend == crate::memory::MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW
        && !semantic_memory_available
    {
        fallback_reason = Some("semantic-memory-flag-off".to_string());
        degradation_markers.push("semantic_memory_feature_disabled".to_string());
    }
    if index_sync_status == "failed" || index_sync_status == "degraded" {
        degradation_markers.push(format!("semantic-memory-sync-{index_sync_status}"));
    }

    let degraded = fallback_reason.is_some() || !degradation_markers.is_empty();

    Ok(MemoryBackendStatus {
        backend_id: requested_backend.clone(),
        default_backend: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
        active_backend: requested_backend.clone(),
        backend_used: if fallback_reason.is_some() {
            MEMORY_BACKEND_GLOSS_LOCAL.to_string()
        } else {
            active_backend
        },
        available: true,
        semantic_memory_feature_enabled,
        semantic_memory_available,
        semantic_memory_path: Some("src-tauri/vendor/semantic-memory".to_string()),
        index_sync_status: index_sync_status.clone(),
        sync_status: index_sync_status,
        last_sync_at: None,
        last_sync_error: link_status.and_then(|status| status.last_sync_error),
        last_retrieval_receipt_id: None,
        last_receipt_ref: None,
        fallback_reason: fallback_reason.clone(),
        degradation_markers,
        backend_version_or_digest: None,
        degraded,
        diagnostic: fallback_reason,
    })
}

#[tauri::command]
pub async fn semantic_memory_link_status(
    notebook_id: String,
    state: State<'_, AppState>,
) -> Result<SemanticMemoryLinkStatus, GlossError> {
    link_status_for_notebook(&state, &notebook_id)
}

#[tauri::command]
pub async fn semantic_memory_reindex_source(
    notebook_id: String,
    source_id: String,
    trace_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<crate::memory::IndexSourceReceipt, GlossError> {
    #[cfg(feature = "semantic-memory-backend")]
    {
        let db_path = state.notebook_db_path(&notebook_id)?;
        let runtime_config = semantic_memory_runtime_config_from_state(&state)?;
        semantic_memory_adapter::reindex_source(
            &state.data_dir,
            &notebook_id,
            &db_path,
            &source_id,
            trace_id,
            Some(runtime_config),
        )
        .await
    }

    #[cfg(not(feature = "semantic-memory-backend"))]
    {
        let _ = (notebook_id, source_id, trace_id, state);
        Err(GlossError::Config(
            "semantic-memory-backend feature is not enabled".to_string(),
        ))
    }
}

#[tauri::command]
pub async fn semantic_memory_reindex_notebook(
    notebook_id: String,
    state: State<'_, AppState>,
) -> Result<SemanticMemoryBackfillReceipt, GlossError> {
    semantic_memory_backfill_notebook(notebook_id, state).await
}

#[tauri::command]
pub async fn semantic_memory_backfill_notebook(
    notebook_id: String,
    state: State<'_, AppState>,
) -> Result<SemanticMemoryBackfillReceipt, GlossError> {
    #[cfg(not(feature = "semantic-memory-backend"))]
    {
        let _ = (notebook_id, state);
        Err(GlossError::Config(
            "semantic-memory-backend feature is not enabled".to_string(),
        ))
    }

    #[cfg(feature = "semantic-memory-backend")]
    {
        let sources = state.with_notebook_db(&notebook_id, |db| db.list_sources())?;
        let runtime_config = semantic_memory_runtime_config_from_state(&state)?;
        let db_path = state.notebook_db_path(&notebook_id)?;
        let receipt_id = uuid::Uuid::new_v4().to_string();
        let mut projected_sources = 0usize;
        let mut skipped_no_chunks = 0usize;
        let mut failed_sources = 0usize;
        let mut chunk_bearing_sources = 0usize;
        let mut total_chunks = 0usize;
        let mut projected_chunks = 0usize;
        let mut errors = Vec::new();

        for source in &sources {
            let chunk_count = state.with_notebook_db(&notebook_id, |db| {
                db.get_chunks_for_source(&source.id)
                    .map(|chunks| chunks.len())
            })?;
            total_chunks += chunk_count;
            if chunk_count > 0 {
                chunk_bearing_sources += 1;
            }

            match semantic_memory_adapter::reindex_source_with_options(
                &state.data_dir,
                &notebook_id,
                &db_path,
                &source.id,
                Some(uuid::Uuid::new_v4().to_string()),
                Some(runtime_config.clone()),
                semantic_memory_adapter::ReindexSourceOptions {
                    rebuild_vector_artifacts: false,
                    classify_zero_chunks_as_skip: true,
                },
            )
            .await
            {
                Ok(receipt) => {
                    if receipt.sync_status == "skipped_no_chunks" {
                        skipped_no_chunks += 1;
                    } else if receipt.sync_status == "synced" {
                        projected_sources += 1;
                        projected_chunks += receipt.indexed_chunks;
                    }
                }
                Err(error) => {
                    failed_sources += 1;
                    errors.push(SemanticMemorySourceProjectionError {
                        source_id: source.id.clone(),
                        title: source.title.clone(),
                        error: error.to_string(),
                    });
                }
            }
        }

        let vector_artifact_receipt = if runtime_config.turbo_quant_enabled {
            semantic_memory_adapter::rebuild_vector_artifacts(
                &state.data_dir,
                &notebook_id,
                Some(runtime_config),
            )
            .await?
        } else {
            None
        };
        if let Some(receipt) = vector_artifact_receipt.as_ref() {
            record_vector_artifact_receipt(&state, &notebook_id, receipt)?;
        }
        let projection_summary = state.with_notebook_db(&notebook_id, |db| {
            let resolved = SourceScope::All.resolve(&sources);
            db.semantic_memory_projection_summary(&notebook_id, &resolved)
        })?;

        Ok(SemanticMemoryBackfillReceipt {
            notebook_id,
            receipt_id,
            total_sources: sources.len(),
            chunk_bearing_sources,
            projected_sources,
            skipped_no_chunks,
            failed_sources,
            stale_sources: projection_summary.stale_sources,
            total_chunks,
            projected_chunks,
            errors,
            vector_artifact_receipt,
            projection_summary,
        })
    }
}

#[tauri::command]
pub async fn semantic_memory_rebuild_vector_artifacts(
    notebook_id: String,
    state: State<'_, AppState>,
) -> Result<Option<serde_json::Value>, GlossError> {
    #[cfg(feature = "semantic-memory-turbo-quant")]
    {
        let mut runtime_config = semantic_memory_runtime_config_from_state(&state)?;
        runtime_config.turbo_quant_enabled = true;
        let receipt = semantic_memory_adapter::rebuild_vector_artifacts(
            &state.data_dir,
            &notebook_id,
            Some(runtime_config),
        )
        .await?;
        if let Some(receipt_value) = receipt.as_ref() {
            record_vector_artifact_receipt(&state, &notebook_id, receipt_value)?;
        }
        Ok(receipt)
    }

    #[cfg(not(feature = "semantic-memory-turbo-quant"))]
    {
        let _ = (notebook_id, state);
        Err(GlossError::Config(
            "semantic-memory-turbo-quant feature is not enabled".to_string(),
        ))
    }
}

#[tauri::command]
pub async fn semantic_memory_vector_artifact_status(
    notebook_id: String,
    state: State<'_, AppState>,
) -> Result<VectorArtifactStatus, GlossError> {
    let (runtime_turbo_quant_enabled, last_status) = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let runtime = features::turbo_quant_active(&app_db)?;
        drop(app_db);
        let statuses = state.with_notebook_db(&notebook_id, |db| {
            db.list_semantic_memory_projection_statuses(&notebook_id)
        })?;
        (runtime, statuses.into_iter().next())
    };
    let latest_probe = latest_retrieval_probe_turbo_proof(&state, &notebook_id)?;
    let exact_rerank = latest_probe
        .as_ref()
        .map(|(_, exact, _, _, _, _)| *exact)
        .unwrap_or_default();
    let exact_rerank_count = latest_probe
        .as_ref()
        .map(|(_, _, count, _, _, _)| *count)
        .unwrap_or_default();
    let probe_candidate_backend = latest_probe
        .as_ref()
        .and_then(|(_, _, _, backend, _, _)| backend.clone());
    let probe_generation_id = latest_probe
        .as_ref()
        .and_then(|(_, _, _, _, generation_id, _)| generation_id.clone());
    let probe_manifest_digest = latest_probe
        .as_ref()
        .and_then(|(_, _, _, _, _, digest)| digest.clone());
    Ok(VectorArtifactStatus {
        compiled_turbo_quant: cfg!(feature = "semantic-memory-turbo-quant"),
        runtime_turbo_quant_enabled,
        candidate_backend: probe_candidate_backend.or_else(|| {
            runtime_turbo_quant_enabled.then(|| "turbo_quant_candidate_then_exact_f32".to_string())
        }),
        artifact_generation_id: last_status
            .as_ref()
            .and_then(|status| status.artifact_generation_id.clone())
            .or(probe_generation_id),
        vector_artifact_manifest_digest: last_status
            .as_ref()
            .and_then(|status| status.vector_artifact_manifest_digest.clone())
            .or(probe_manifest_digest),
        vector_artifact_missing_count: if runtime_turbo_quant_enabled && last_status.is_none() {
            1
        } else {
            0
        },
        vector_artifact_stale_count: last_status
            .as_ref()
            .is_some_and(|status| status.status == "artifact_stale")
            as usize,
        exact_rerank,
        exact_rerank_count,
        last_receipt_id: last_status
            .as_ref()
            .and_then(|status| status.last_receipt_id.clone()),
        last_error: last_status.and_then(|status| status.last_error),
    })
}

#[tauri::command]
pub async fn compare_memory_backends(
    notebook_id: String,
    query: String,
    source_scope: SourceScope,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<MemoryBackendComparison, GlossError> {
    let request = MemorySearchRequest {
        notebook_id: notebook_id.clone(),
        source_scope,
        query,
        limit: limit.unwrap_or(8),
        trace_id: Some(uuid::Uuid::new_v4().to_string()),
        allow_fallback: false,
    };

    let db_path = state.notebook_db_path(&notebook_id)?;
    #[cfg(feature = "semantic-memory-backend")]
    let semantic_runtime_config = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        features::require_semantic_memory_preview_enabled(&app_db)?;
        semantic_memory_adapter::runtime_config_from_settings(
            app_db.get_setting("semantic_memory_embedding_url")?,
            app_db.get_setting("semantic_memory_embedding_model")?,
            app_db.get_setting("semantic_memory_embedding_timeout_secs")?,
            features::turbo_quant_active(&app_db)?,
            crate::commands::chat::setting_is_enabled(
                app_db
                    .get_setting(features::SEMANTIC_MEMORY_TURBO_QUANT_REQUIRE_FRESH_ARTIFACTS)?,
            ),
        )
    };
    let (all_sources, local_backend_result, local_latency_ms, semantic_links) = {
        let db = crate::db::notebook_db::NotebookDb::connect(&db_path)?;
        let all_sources = db.list_sources()?;
        let local = GlossLocalMemoryBackend::new(notebook_id.clone(), &db, &all_sources);
        let local_started = Instant::now();
        let local_backend_result = local.search(request.clone())?;
        let local_latency_ms = local_started.elapsed().as_millis();
        #[cfg(feature = "semantic-memory-backend")]
        let semantic_links = semantic_memory_adapter::load_links(&db)?;
        #[cfg(not(feature = "semantic-memory-backend"))]
        let semantic_links = Vec::new();
        (
            all_sources,
            local_backend_result,
            local_latency_ms,
            semantic_links,
        )
    };

    compare_memory_backends_for_notebook(
        &state.data_dir,
        &notebook_id,
        &all_sources,
        request,
        local_backend_result,
        local_latency_ms,
        semantic_links,
        #[cfg(feature = "semantic-memory-backend")]
        Some(semantic_runtime_config),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::app_db::AppDb;
    use crate::db::notebook_db::NotebookDb;
    use crate::provider_config_store::SecretStore;
    use crate::providers::ModelRegistry;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU64};
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
            ingestion_active: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            llm_gate: tokio::sync::Semaphore::new(1),
            gpu_gate: tokio::sync::Semaphore::new(1),
            gate_owners: Mutex::new(HashMap::new()),
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
                    available: true,
                    stale: false,
                    last_error: None,
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
                    available: true,
                    stale: false,
                    last_error: None,
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
                        available: true,
                        stale: false,
                        last_error: None,
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
