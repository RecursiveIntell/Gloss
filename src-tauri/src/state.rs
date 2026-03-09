use crate::db::app_db::AppDb;
use crate::db::notebook_db::NotebookDb;
use crate::error::GlossError;
use crate::ingestion::embed::{EmbeddingService, HnswIndex};
use crate::providers::ModelRegistry;
use crate::retrieval::hybrid_search::{self, SearchResult};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::AppHandle;
use tokio::sync::Semaphore;

/// Native semantic indexing (fastembed + usearch) remains an in-process crash
/// vector during ingestion. Keep it disabled until those calls are isolated from
/// the desktop process.
pub const NATIVE_SEMANTIC_INDEXING_ENABLED: bool = false;

/// Global application state managed by Tauri
pub struct AppState {
    /// App-level database (gloss.db)
    pub app_db: Mutex<AppDb>,
    /// Cached notebook database paths keyed by notebook ID.
    /// Each DB access opens its own SQLite connection so long-running ingestion
    /// work in one notebook does not serialize every other read for that same
    /// notebook at the application mutex layer.
    pub notebook_dbs: Mutex<HashMap<String, PathBuf>>,
    /// LLM provider registry
    pub model_registry: Mutex<ModelRegistry>,
    /// Application data directory
    pub data_dir: PathBuf,
    /// Embedding model (lazy-initialized on first use)
    pub embedder: Mutex<Option<EmbeddingService>>,
    /// Per-notebook HNSW vector indices keyed by notebook ID
    pub hnsw_indices: Mutex<HashMap<String, HnswIndex>>,
    /// Whether summary generation is manually paused by the user
    pub summary_paused: AtomicBool,
    /// Number of sources currently being ingested (extract/chunk/embed).
    /// Summary loop yields while this is > 0.
    pub ingestion_active: AtomicU32,

    // --- Scheduling primitives (CLAUDE.md contracts) ---
    /// Single-flight LLM/GPU gate: at most one inference request in-flight.
    /// Acquire before any LLM call (chat, summary, studio).
    pub llm_gate: Semaphore,
    /// GPU memory gate: prevents concurrent ONNX embedding + Ollama inference.
    /// Must be acquired before any GPU-intensive operation (embedding, LLM calls).
    pub gpu_gate: Semaphore,
    /// Currently active notebook ID. Summary worker idles when None.
    pub active_notebook_id: Mutex<Option<String>>,
    /// Epoch counter incremented on notebook switch. Used for soft-cancel of
    /// summary jobs queued for a previous notebook.
    pub active_epoch: AtomicU64,
    /// Chat grace window: epoch millis until which summaries must not start.
    /// Set to now+15s on each user message; reset by bump_chat_grace().
    pub chat_grace_until: Mutex<u64>,
    /// Last user-initiated action (epoch millis). Used to detect idle state
    /// for auto-summarization. Bumped by send_message, set_active_notebook, etc.
    pub last_user_activity: Mutex<u64>,
}

impl AppState {
    /// Initialize application state on startup.
    pub fn initialize(_app_handle: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        let data_dir = directories::ProjectDirs::from("com", "sikmindz", "Gloss")
            .ok_or_else(|| GlossError::Config("Could not determine data directory".into()))?
            .data_dir()
            .to_path_buf();

        std::fs::create_dir_all(&data_dir)?;
        std::fs::create_dir_all(data_dir.join("notebooks"))?;

        let db_path = data_dir.join("gloss.db");
        let app_db = AppDb::open(&db_path)?;

        let model_registry = ModelRegistry::new(&app_db)?;

        tracing::info!(data_dir = %data_dir.display(), "Gloss initialized");

        Ok(Self {
            app_db: Mutex::new(app_db),
            notebook_dbs: Mutex::new(HashMap::new()),
            model_registry: Mutex::new(model_registry),
            data_dir,
            embedder: Mutex::new(None),
            hnsw_indices: Mutex::new(HashMap::new()),
            summary_paused: AtomicBool::new(false),
            ingestion_active: AtomicU32::new(0),
            llm_gate: Semaphore::new(1),
            gpu_gate: Semaphore::new(1),
            active_notebook_id: Mutex::new(None),
            active_epoch: AtomicU64::new(0),
            chat_grace_until: Mutex::new(0),
            last_user_activity: Mutex::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
            ),
        })
    }

    /// Ensure the embedding model is initialized. Returns an error message on failure.
    /// Emits status events for UI feedback (Fix 8).
    pub fn ensure_embedder(&self, app_handle: Option<&tauri::AppHandle>) -> Result<(), GlossError> {
        let mut embedder = self
            .embedder
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;

        if embedder.is_some() {
            return Ok(());
        }

        // Notify frontend
        if let Some(handle) = app_handle {
            use tauri::Emitter;
            let _ = handle.emit(
                "status:embedding_model",
                serde_json::json!({
                    "state": "downloading",
                    "message": "Loading embedding model (first time may download ~100MB)…"
                }),
            );
        }

        tracing::info!("Initializing embedding model…");
        let cache_dir = self.data_dir.join("models");
        std::fs::create_dir_all(&cache_dir)?;
        let service = EmbeddingService::new(&cache_dir)?;
        *embedder = Some(service);

        if let Some(handle) = app_handle {
            use tauri::Emitter;
            let _ = handle.emit(
                "status:embedding_model",
                serde_json::json!({
                    "state": "ready",
                    "message": "Embedding model loaded"
                }),
            );
        }

        tracing::info!("Embedding model ready");
        Ok(())
    }

    /// Get or create the HNSW index for a notebook.
    /// Queries the notebook DB for the max embedding_id to avoid label collisions
    /// after vector deletions (where index.size() < max label ever assigned).
    ///
    /// Gathers external data without holding hnsw_indices (avoids lock-ordering
    /// deadlocks), then creates the index INSIDE the hnsw_indices lock with a
    /// second contains_key guard to prevent the race where two threads both
    /// create a usearch Index and one is immediately dropped (corrupts C++ heap).
    pub fn ensure_hnsw_index(&self, notebook_id: &str) -> Result<(), GlossError> {
        // Quick check — avoids unnecessary work if index already loaded
        {
            let indices = self
                .hnsw_indices
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            if indices.contains_key(notebook_id) {
                return Ok(());
            }
        }
        // hnsw_indices released here — safe to gather notebook metadata without
        // nested lock risk.

        // Gather data needed for index creation (requires other locks)
        let nb_dir = {
            let app_db = self
                .app_db
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            let nb = app_db.get_notebook(notebook_id)?;
            PathBuf::from(nb.directory)
        };

        let max_embedding_id = self.with_notebook_db(notebook_id, |db| db.max_embedding_id())?;

        // Re-acquire hnsw_indices and create the index INSIDE the lock.
        // Critical: the second contains_key check prevents the race where two
        // threads both passed the first check. Without this, two usearch C++
        // Index objects get created and one is immediately dropped, corrupting
        // the C++ heap (manifests as "free(): corrupted unsorted chunks").
        let mut indices = self
            .hnsw_indices
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        if indices.contains_key(notebook_id) {
            return Ok(()); // Another thread beat us — nothing to drop
        }

        let index_path = nb_dir.join("embeddings").join("chunks.usearch");
        let index = if index_path.exists() {
            tracing::debug!(
                notebook_id,
                ?max_embedding_id,
                "Loading existing HNSW index"
            );
            HnswIndex::load_with_hwm(&index_path, max_embedding_id)?
        } else {
            std::fs::create_dir_all(nb_dir.join("embeddings"))?;
            tracing::debug!(notebook_id, "Creating new HNSW index");
            HnswIndex::new()?
        };

        indices.insert(notebook_id.to_string(), index);
        Ok(())
    }

    /// Save the HNSW index for a notebook to disk.
    pub fn save_hnsw_index(&self, notebook_id: &str) -> Result<(), GlossError> {
        let indices = self
            .hnsw_indices
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;

        if let Some(index) = indices.get(notebook_id) {
            let nb_dir = {
                let app_db = self
                    .app_db
                    .lock()
                    .map_err(|e| GlossError::Other(e.to_string()))?;
                let nb = app_db.get_notebook(notebook_id)?;
                PathBuf::from(nb.directory)
            };
            let index_path = nb_dir.join("embeddings").join("chunks.usearch");
            index.save(&index_path)?;
        }
        Ok(())
    }

    // --- Scheduling helpers ---

    /// Set the active notebook (or None to deselect). Increments epoch.
    pub fn set_active_notebook(&self, id: Option<String>) {
        {
            let mut active = self
                .active_notebook_id
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *active = id;
        }
        self.active_epoch.fetch_add(1, Ordering::SeqCst);
        self.bump_user_activity();
        tracing::debug!(
            epoch = self.active_epoch.load(Ordering::SeqCst),
            "Active notebook changed"
        );
    }

    /// Get the currently active notebook ID (cloned).
    pub fn get_active_notebook_id(&self) -> Option<String> {
        self.active_notebook_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Get the current epoch value.
    pub fn get_active_epoch(&self) -> u64 {
        self.active_epoch.load(Ordering::SeqCst)
    }

    /// Returns true when the notebook/epoch pair is still the active one.
    pub fn is_active_notebook_epoch(&self, notebook_id: &str, epoch: u64) -> bool {
        self.get_active_notebook_id().as_deref() == Some(notebook_id)
            && self.get_active_epoch() == epoch
    }

    /// Bump the chat grace window to now + 15 seconds.
    pub fn bump_chat_grace(&self) {
        let until = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
            + 15_000;
        let mut grace = self
            .chat_grace_until
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *grace = until;
    }

    /// Check whether we are inside the chat grace window.
    pub fn is_in_chat_grace(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let grace = self
            .chat_grace_until
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        now < *grace
    }

    /// Record a user-initiated action (bumps activity timestamp).
    pub fn bump_user_activity(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut last = self
            .last_user_activity
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *last = now;
    }

    /// Returns how many seconds since the last user-initiated action.
    pub fn idle_seconds(&self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let last = self
            .last_user_activity
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        now.saturating_sub(*last) / 1000
    }

    fn notebook_db_path(&self, notebook_id: &str) -> Result<PathBuf, GlossError> {
        {
            let dbs = self
                .notebook_dbs
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            if let Some(path) = dbs.get(notebook_id) {
                return Ok(path.clone());
            }
        }

        let db_path = {
            let app_db = self
                .app_db
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            let notebook = app_db.get_notebook(notebook_id)?;
            PathBuf::from(notebook.directory).join("notebook.db")
        };

        let mut dbs = self
            .notebook_dbs
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        dbs.entry(notebook_id.to_string())
            .or_insert_with(|| db_path.clone());
        Ok(db_path)
    }

    /// Execute a function with a notebook database connection.
    /// A fresh SQLite connection is opened per call so readers are not blocked
    /// behind long-running work on a shared application mutex.
    pub fn with_notebook_db<F, T>(&self, notebook_id: &str, f: F) -> Result<T, GlossError>
    where
        F: FnOnce(&NotebookDb) -> Result<T, GlossError>,
    {
        let db_path = self.notebook_db_path(notebook_id)?;
        let db = NotebookDb::connect(&db_path)?;
        f(&db)
    }

    /// Try to perform hybrid search using HNSW + FTS5. Returns `Ok(None)` if
    /// the embedder or HNSW index is not available (e.g., no embeddings yet).
    pub fn try_hybrid_search(
        &self,
        notebook_id: &str,
        query: &str,
        selected_source_ids: &[String],
        top_k: usize,
    ) -> Result<Option<Vec<SearchResult>>, GlossError> {
        if !NATIVE_SEMANTIC_INDEXING_ENABLED {
            return Ok(None);
        }

        // Use try_lock to avoid blocking if locks are held by ingestion
        let embedder_guard = match self.embedder.try_lock() {
            Ok(g) => g,
            Err(_) => {
                tracing::warn!("Embedder lock busy during search, falling back to raw context");
                return Ok(None);
            }
        };
        let embedder = match embedder_guard.as_ref() {
            Some(e) => e,
            None => return Ok(None),
        };

        let indices_guard = match self.hnsw_indices.try_lock() {
            Ok(g) => g,
            Err(_) => {
                tracing::warn!("HNSW index lock busy during search, falling back to raw context");
                return Ok(None);
            }
        };
        let index = match indices_guard.get(notebook_id) {
            Some(i) => i,
            None => return Ok(None),
        };

        let db_path = self.notebook_db_path(notebook_id)?;
        let nb_db = NotebookDb::connect(&db_path)?;
        if !nb_db.can_run_hybrid_search(selected_source_ids)? {
            tracing::debug!(
                notebook_id,
                selected = selected_source_ids.len(),
                "Hybrid search skipped because the selected scope is not fully indexed"
            );
            return Ok(None);
        }

        let results = hybrid_search::hybrid_search(
            query,
            &nb_db,
            embedder,
            index,
            selected_source_ids,
            top_k,
        )?;
        Ok(Some(results))
    }
}
