use crate::db::app_db::AppDb;
use crate::db::notebook_db::NotebookDb;
use crate::error::GlossError;
use crate::ingestion::embed::{EmbeddingService, HnswIndex};
use crate::providers::ModelRegistry;
use crate::retrieval::hybrid_search::{self, SearchResult};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::AppHandle;
use tokio::sync::Semaphore;

/// Global application state managed by Tauri
pub struct AppState {
    /// App-level database (gloss.db)
    pub app_db: Mutex<AppDb>,
    /// Open notebook databases keyed by notebook ID
    pub notebook_dbs: Mutex<HashMap<String, NotebookDb>>,
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

    // --- Scheduling primitives (CLAUDE.md contracts) ---

    /// Single-flight LLM/GPU gate: at most one inference request in-flight.
    /// Acquire before any LLM call (chat, summary, studio).
    pub llm_gate: Semaphore,
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
            llm_gate: Semaphore::new(1),
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
    pub fn ensure_hnsw_index(&self, notebook_id: &str) -> Result<(), GlossError> {
        let mut indices = self
            .hnsw_indices
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;

        if indices.contains_key(notebook_id) {
            return Ok(());
        }

        // Try to load existing index from disk
        let nb_dir = {
            let app_db = self
                .app_db
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            let nb = app_db.get_notebook(notebook_id)?;
            PathBuf::from(nb.directory)
        };

        let index_path = nb_dir.join("embeddings").join("chunks.usearch");
        let index = if index_path.exists() {
            tracing::debug!(notebook_id, "Loading existing HNSW index");
            HnswIndex::load(&index_path)?
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
            let mut active = self.active_notebook_id.lock().unwrap();
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
        self.active_notebook_id.lock().unwrap().clone()
    }

    /// Get the current epoch value.
    pub fn get_active_epoch(&self) -> u64 {
        self.active_epoch.load(Ordering::SeqCst)
    }

    /// Bump the chat grace window to now + 15 seconds.
    pub fn bump_chat_grace(&self) {
        let until = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
            + 15_000;
        let mut grace = self.chat_grace_until.lock().unwrap();
        *grace = until;
    }

    /// Check whether we are inside the chat grace window.
    pub fn is_in_chat_grace(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let grace = self.chat_grace_until.lock().unwrap();
        now < *grace
    }

    /// Record a user-initiated action (bumps activity timestamp).
    pub fn bump_user_activity(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut last = self.last_user_activity.lock().unwrap();
        *last = now;
    }

    /// Returns how many seconds since the last user-initiated action.
    pub fn idle_seconds(&self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let last = self.last_user_activity.lock().unwrap();
        now.saturating_sub(*last) / 1000
    }

    /// Get or open a notebook database.
    pub fn get_notebook_db(&self, notebook_id: &str) -> Result<(), GlossError> {
        let mut dbs = self.notebook_dbs.lock().map_err(|e| GlossError::Other(e.to_string()))?;
        if !dbs.contains_key(notebook_id) {
            let app_db = self.app_db.lock().map_err(|e| GlossError::Other(e.to_string()))?;
            let notebook = app_db.get_notebook(notebook_id)?;
            let nb_dir = PathBuf::from(&notebook.directory);
            let db_path = nb_dir.join("notebook.db");
            let nb_db = NotebookDb::open(&db_path)?;
            dbs.insert(notebook_id.to_string(), nb_db);
        }
        Ok(())
    }

    /// Execute a function with a locked notebook database.
    pub fn with_notebook_db<F, T>(&self, notebook_id: &str, f: F) -> Result<T, GlossError>
    where
        F: FnOnce(&NotebookDb) -> Result<T, GlossError>,
    {
        self.get_notebook_db(notebook_id)?;
        let dbs = self.notebook_dbs.lock().map_err(|e| GlossError::Other(e.to_string()))?;
        let db = dbs.get(notebook_id).ok_or_else(|| {
            GlossError::NotFound(format!("Notebook {} not opened", notebook_id))
        })?;
        f(db)
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
        // Acquire all three locks — all sync, no await
        let embedder_guard = self
            .embedder
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let embedder = match embedder_guard.as_ref() {
            Some(e) => e,
            None => return Ok(None),
        };

        let indices_guard = self
            .hnsw_indices
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let index = match indices_guard.get(notebook_id) {
            Some(i) => i,
            None => return Ok(None),
        };

        let dbs_guard = self
            .notebook_dbs
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let nb_db = match dbs_guard.get(notebook_id) {
            Some(db) => db,
            None => return Ok(None),
        };

        let results =
            hybrid_search::hybrid_search(query, nb_db, embedder, index, selected_source_ids, top_k)?;
        Ok(Some(results))
    }
}
