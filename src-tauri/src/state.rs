use crate::db::app_db::AppDb;
use crate::db::notebook_db::NotebookDb;
use crate::error::GlossError;
use crate::features;
use crate::ingestion::embed::{EmbeddingService, HnswIndex};
use crate::memory::types::RetrievalOutcome;
use crate::provider_config_store::SecretStore;
use crate::providers::ModelRegistry;
use crate::retrieval::hybrid_search;
use crate::retrieval::source_scope::ResolvedSourceScope;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::AppHandle;
use tokio::sync::Semaphore;

/// Native semantic indexing (fastembed + usearch) remains an in-process crash
/// vector during ingestion. Keep it disabled until those calls are isolated from
/// the desktop process.
pub const NATIVE_SEMANTIC_INDEXING_ENABLED: bool = false;
pub const SUMMARY_MODE_AUTO: &str = "auto";
pub const SUMMARY_MODE_MANUAL: &str = "manual";

#[derive(Debug, Clone, serde::Serialize)]
pub struct RuntimeGateOwner {
    pub gate: String,
    pub owner: String,
    pub detail: String,
    pub since_ms: u64,
}

/// Global application state managed by Tauri
pub struct AppState {
    /// App-level database (gloss.db)
    pub app_db: Mutex<AppDb>,
    /// Encrypted local secret storage for provider API keys.
    pub secret_store: SecretStore,
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
    pub ingestion_active: Arc<AtomicU32>,

    // --- Scheduling primitives (CLAUDE.md contracts) ---
    /// Single-flight LLM/GPU gate: at most one inference request in-flight.
    /// Acquire before any LLM call (chat, summary, studio).
    pub llm_gate: Semaphore,
    /// GPU memory gate: prevents concurrent ONNX embedding + Ollama inference.
    /// Must be acquired before any GPU-intensive operation (embedding, LLM calls).
    pub gpu_gate: Semaphore,
    /// Current runtime gate owners for user-visible contention diagnostics.
    pub gate_owners: Mutex<HashMap<String, RuntimeGateOwner>>,
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
    fn summary_mode_starts_paused(app_db: &AppDb) -> Result<bool, GlossError> {
        let summary_mode = app_db
            .get_setting("summary_mode")?
            .unwrap_or_else(|| SUMMARY_MODE_MANUAL.to_string());
        Ok(summary_mode != SUMMARY_MODE_AUTO)
    }

    pub fn summary_mode_is_auto(&self) -> Result<bool, GlossError> {
        let app_db = self
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        Ok(app_db
            .get_setting("summary_mode")?
            .unwrap_or_else(|| SUMMARY_MODE_MANUAL.to_string())
            == SUMMARY_MODE_AUTO)
    }

    fn migrate_legacy_secrets(
        app_db: &AppDb,
        secret_store: &SecretStore,
    ) -> Result<(), GlossError> {
        for setting_key in ["openai_api_key", "anthropic_api_key"] {
            if let Some(value) = app_db
                .get_setting(setting_key)?
                .filter(|value| !value.is_empty())
            {
                secret_store.set(setting_key, Some(&value))?;
                app_db.set_setting(setting_key, "")?;
            }
        }

        for (provider_id, setting_key) in [
            ("openai", "openai_api_key"),
            ("anthropic", "anthropic_api_key"),
        ] {
            if let Some(value) = app_db
                .get_provider_api_key(provider_id)?
                .filter(|value| !value.is_empty())
            {
                if !secret_store.contains(setting_key)? {
                    secret_store.set(setting_key, Some(&value))?;
                }
                app_db.clear_provider_api_key(provider_id)?;
            }
        }

        Ok(())
    }

    fn reconcile_notebook_source_counts(app_db: &AppDb) -> Result<(), GlossError> {
        let notebooks = app_db.list_notebooks()?;
        for notebook in notebooks {
            let notebook_db_path = PathBuf::from(&notebook.directory).join("notebook.db");
            if !notebook_db_path.exists() {
                tracing::warn!(
                    notebook_id = %notebook.id,
                    path = %notebook_db_path.display(),
                    "Skipping source-count reconcile for missing notebook DB"
                );
                continue;
            }

            let count =
                match NotebookDb::connect(&notebook_db_path).and_then(|db| db.source_count()) {
                    Ok(count) => count,
                    Err(e) => {
                        tracing::warn!(
                            notebook_id = %notebook.id,
                            path = %notebook_db_path.display(),
                            error = %e,
                            "Failed to reconcile notebook source count"
                        );
                        continue;
                    }
                };

            if count != notebook.source_count {
                app_db.update_source_count(&notebook.id, count)?;
                tracing::info!(
                    notebook_id = %notebook.id,
                    old_count = notebook.source_count,
                    new_count = count,
                    "Reconciled notebook source count"
                );
            }
        }

        Ok(())
    }

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
        if app_db.get_setting("memory_backend")?.is_none() {
            app_db.set_setting("memory_backend", "gloss-local")?;
        }
        if app_db.get_setting("memory_backend_fallback")?.is_none() {
            app_db.set_setting("memory_backend_fallback", "true")?;
        }
        if app_db
            .get_setting("semantic_memory_embedding_url")?
            .is_none()
        {
            app_db.set_setting("semantic_memory_embedding_url", "http://localhost:11434")?;
        }
        if app_db
            .get_setting("semantic_memory_embedding_model")?
            .is_none()
        {
            app_db.set_setting("semantic_memory_embedding_model", "nomic-embed-text")?;
        }
        if app_db
            .get_setting("semantic_memory_embedding_timeout_secs")?
            .is_none()
        {
            app_db.set_setting("semantic_memory_embedding_timeout_secs", "10")?;
        }
        if app_db
            .get_setting("semantic_memory_search_timeout_ms")?
            .is_none()
        {
            app_db.set_setting("semantic_memory_search_timeout_ms", "8000")?;
        }
        features::ensure_default_feature_settings(&app_db)?;
        let secret_store = SecretStore::new(&data_dir)?;
        Self::migrate_legacy_secrets(&app_db, &secret_store)?;
        Self::reconcile_notebook_source_counts(&app_db)?;

        let model_registry = ModelRegistry::new(&app_db, &secret_store)?;
        let summary_starts_paused = Self::summary_mode_starts_paused(&app_db)?;

        tracing::info!(data_dir = %data_dir.display(), "Gloss initialized");

        Ok(Self {
            app_db: Mutex::new(app_db),
            secret_store,
            notebook_dbs: Mutex::new(HashMap::new()),
            model_registry: Mutex::new(model_registry),
            data_dir,
            embedder: Mutex::new(None),
            hnsw_indices: Mutex::new(HashMap::new()),
            summary_paused: AtomicBool::new(summary_starts_paused),
            ingestion_active: Arc::new(AtomicU32::new(0)),
            llm_gate: Semaphore::new(1),
            gpu_gate: Semaphore::new(1),
            gate_owners: Mutex::new(HashMap::new()),
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

    /// Set the active notebook (or None to deselect).
    ///
    /// Returns true only when the active notebook actually changed. Callers may
    /// optionally supply a target epoch so pending queue work from a previous
    /// app session can be resumed instead of being cancelled as stale on the
    /// first post-restart activation.
    pub fn set_active_notebook(&self, id: Option<String>, epoch: Option<u64>) -> bool {
        let mut active = self
            .active_notebook_id
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        if *active == id {
            return false;
        }

        *active = id;
        drop(active);

        match epoch {
            Some(epoch) => self.active_epoch.store(epoch, Ordering::SeqCst),
            None => {
                self.active_epoch.fetch_add(1, Ordering::SeqCst);
            }
        }

        self.bump_user_activity();
        tracing::debug!(
            active_notebook_id = ?self.get_active_notebook_id(),
            epoch = self.active_epoch.load(Ordering::SeqCst),
            "Active notebook changed"
        );
        true
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

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    pub fn set_gate_owner(&self, gate: &str, owner: &str, detail: &str) {
        let mut owners = self.gate_owners.lock().unwrap_or_else(|e| e.into_inner());
        owners.insert(
            gate.to_string(),
            RuntimeGateOwner {
                gate: gate.to_string(),
                owner: owner.to_string(),
                detail: detail.to_string(),
                since_ms: Self::now_ms(),
            },
        );
    }

    pub fn clear_gate_owner(&self, gate: &str, owner: &str) {
        let mut owners = self.gate_owners.lock().unwrap_or_else(|e| e.into_inner());
        if owners
            .get(gate)
            .map(|current| current.owner == owner)
            .unwrap_or(false)
        {
            owners.remove(gate);
        }
    }

    pub fn gate_owners_snapshot(&self) -> Vec<RuntimeGateOwner> {
        self.gate_owners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
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

    pub(crate) fn notebook_db_path(&self, notebook_id: &str) -> Result<PathBuf, GlossError> {
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

    /// Build a truthful local retrieval outcome. BM25/FTS5 always remains the
    /// stable baseline; native dense retrieval contributes only when available.
    pub fn local_retrieval_outcome(
        &self,
        notebook_id: &str,
        query: &str,
        scope: &ResolvedSourceScope,
        top_k: usize,
        trace_ref: String,
    ) -> Result<RetrievalOutcome, GlossError> {
        let db_path = self.notebook_db_path(notebook_id)?;
        let nb_db = NotebookDb::connect(&db_path)?;
        let embedder_guard = self.embedder.try_lock().ok();
        let embedder = embedder_guard.as_ref().and_then(|guard| guard.as_ref());
        let indices_guard = self.hnsw_indices.try_lock().ok();
        let index = indices_guard
            .as_ref()
            .and_then(|indices| indices.get(notebook_id));
        hybrid_search::local_retrieval_outcome(
            query,
            &nb_db,
            embedder,
            index,
            NATIVE_SEMANTIC_INDEXING_ENABLED,
            scope,
            top_k,
            trace_ref,
        )
    }
}

/// RAII guard for background activity counters that must not remain elevated
/// after an early return, panic unwind, or dropped async task.
pub struct ActiveCounterGuard {
    counter: Arc<AtomicU32>,
    label: &'static str,
    active: bool,
}

impl ActiveCounterGuard {
    pub fn new(counter: &Arc<AtomicU32>, label: &'static str) -> Self {
        let active = counter
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                current.checked_add(1)
            })
            .is_ok();
        if !active {
            tracing::warn!(
                counter = label,
                "Background activity counter guard could not increment saturated counter"
            );
        }
        Self {
            counter: Arc::clone(counter),
            label,
            active,
        }
    }
}

impl Drop for ActiveCounterGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        if self
            .counter
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                current.checked_sub(1)
            })
            .is_err()
        {
            self.active = false;
            tracing::warn!(
                counter = self.label,
                "Background activity counter finalizer found an already-zero counter"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::notebook_db::Source;
    use tempfile::tempdir;

    #[test]
    fn active_counter_guard_finalizes_on_drop_and_saturates() {
        let counter = Arc::new(AtomicU32::new(0));
        {
            let _guard = ActiveCounterGuard::new(&counter, "test");
            assert_eq!(counter.load(Ordering::SeqCst), 1);
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0);

        {
            let guard = ActiveCounterGuard::new(&counter, "test");
            counter.store(0, Ordering::SeqCst);
            drop(guard);
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0);

        counter.store(u32::MAX, Ordering::SeqCst);
        {
            let _guard = ActiveCounterGuard::new(&counter, "test");
            assert_eq!(counter.load(Ordering::SeqCst), u32::MAX);
        }
        assert_eq!(counter.load(Ordering::SeqCst), u32::MAX);
    }

    #[test]
    fn test_reconcile_notebook_source_counts() {
        let dir = tempdir().unwrap();
        let app_db_path = dir.path().join("gloss.db");
        let app_db = AppDb::open(&app_db_path).unwrap();

        let notebook_dir = dir.path().join("notebooks").join("nb1");
        std::fs::create_dir_all(notebook_dir.join("sources")).unwrap();
        let notebook_dir_str = notebook_dir.to_string_lossy().to_string();

        app_db
            .create_notebook("nb1", "Count Drift", &notebook_dir_str)
            .unwrap();
        app_db.update_source_count("nb1", 99).unwrap();

        let notebook_db = NotebookDb::open(&notebook_dir.join("notebook.db")).unwrap();
        for id in ["s1", "s2"] {
            notebook_db
                .insert_source(&Source {
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
                })
                .unwrap();
        }

        AppState::reconcile_notebook_source_counts(&app_db).unwrap();

        let notebook = app_db.get_notebook("nb1").unwrap();
        assert_eq!(notebook.source_count, 2);
    }

    #[test]
    fn test_migrate_legacy_secrets_scrubs_sqlite() {
        let dir = tempdir().unwrap();
        let app_db_path = dir.path().join("gloss.db");
        let app_db = AppDb::open(&app_db_path).unwrap();
        let secret_store = SecretStore::new(dir.path()).unwrap();

        app_db.set_setting("openai_api_key", "sk-settings").unwrap();
        app_db
            .conn
            .execute(
                "INSERT OR REPLACE INTO providers (id, enabled, base_url, api_key) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params!["anthropic", 1, "https://api.anthropic.com/v1", "sk-provider"],
            )
            .unwrap();

        AppState::migrate_legacy_secrets(&app_db, &secret_store).unwrap();

        assert_eq!(
            secret_store.get("openai_api_key").unwrap(),
            Some("sk-settings".to_string())
        );
        assert_eq!(
            secret_store.get("anthropic_api_key").unwrap(),
            Some("sk-provider".to_string())
        );
        assert_eq!(
            app_db.get_setting("openai_api_key").unwrap(),
            Some(String::new())
        );
        assert_eq!(app_db.get_provider_api_key("anthropic").unwrap(), None);
    }

    #[test]
    fn summary_mode_defaults_to_manual_startup() {
        let dir = tempdir().unwrap();
        let app_db_path = dir.path().join("gloss.db");
        let app_db = AppDb::open(&app_db_path).unwrap();

        assert!(AppState::summary_mode_starts_paused(&app_db).unwrap());

        app_db
            .set_setting("summary_mode", SUMMARY_MODE_AUTO)
            .unwrap();
        assert!(!AppState::summary_mode_starts_paused(&app_db).unwrap());

        app_db
            .set_setting("summary_mode", SUMMARY_MODE_MANUAL)
            .unwrap();
        assert!(AppState::summary_mode_starts_paused(&app_db).unwrap());
    }
}
