use crate::db::app_db::AppDb;
use crate::db::notebook_db::{EmbeddingIndexMetadata, NotebookDb, NATIVE_HNSW_INDEX_ID};
use crate::db::notebook_pool::NotebookDbPools;
use crate::error::GlossError;
use crate::features;
use crate::ingestion::embed::{EmbeddingService, HnswIndex};
use crate::memory::types::{RetrievalOutcome, RetrievalReasonCode};
use crate::provider_config_store::SecretStore;
use crate::providers::ModelRegistry;
use crate::redaction::redact_path;
use crate::retrieval::hybrid_search;
use crate::retrieval::source_scope::ResolvedSourceScope;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::AppHandle;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

const CHAT_STREAM_REPLAY_CAPACITY: usize = 4096;

/// Release builds keep native dense indexing enabled. Ingestion still runs
/// through bounded single-source work and the GPU gate so fallback/degradation
/// is visible instead of silently skipping dense vectors.
pub const NATIVE_SEMANTIC_INDEXING_ENABLED: bool = true;
/// LRU cache of (query_text, model_id) -> embedding vector. Bounded.
/// Backing store: HashMap of text -> (Vec<f32>, generation). On insert,
/// the oldest generation is evicted. This is O(1) amortized and bounded
/// to `MAX_ENTRIES` (~384KB at 256 entries * 384 dims * 4 bytes).
#[derive(Debug)]
pub struct QueryEmbedCache {
    entries: HashMap<String, (Vec<f32>, u64)>,
    generation: u64,
    max_entries: usize,
    hits: u64,
    misses: u64,
}

impl Default for QueryEmbedCache {
    fn default() -> Self {
        Self::new(256)
    }
}

impl QueryEmbedCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: HashMap::new(),
            generation: 0,
            max_entries: max_entries.max(1),
            hits: 0,
            misses: 0,
        }
    }

    pub fn get(&mut self, key: &str) -> Option<Vec<f32>> {
        match self.entries.get(key) {
            Some((vec, _gen)) => {
                self.hits += 1;
                Some(vec.clone())
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    pub fn insert(&mut self, key: String, vec: Vec<f32>) {
        if self.entries.len() >= self.max_entries && !self.entries.contains_key(&key) {
            // Evict all entries from the oldest generation. A new generation
            // is bumped; everything before it is stale. This is the simplest
            // bounded-LRU that doesn't require a linked list.
            self.generation += 1;
            let keep = self.generation;
            self.entries.retain(|_, (_, gen)| *gen == keep);
            // If we still somehow overflowed (all entries were fresh), keep
            // half to be safe.
            if self.entries.len() >= self.max_entries {
                let drop = self.entries.len() - self.max_entries / 2;
                let keys: Vec<String> = self.entries.keys().take(drop).cloned().collect();
                for k in keys {
                    self.entries.remove(&k);
                }
            }
        }
        let gen = self.generation;
        self.entries.insert(key, (vec, gen));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.generation = 0;
    }

    #[cfg(test)]
    pub fn stats(&self) -> (u64, u64, usize) {
        (self.hits, self.misses, self.entries.len())
    }
}

#[cfg(test)]
mod query_embed_cache_tests {
    use super::*;

    #[test]
    fn lru_evicts_oldest_generation() {
        let mut c = QueryEmbedCache::new(2);
        c.insert("a".into(), vec![1.0]);
        c.insert("b".into(), vec![2.0]);
        assert_eq!(c.entries.len(), 2);
        // Force eviction
        c.insert("c".into(), vec![3.0]);
        assert!(c.entries.len() <= 2);
        // At least one of the originals should be gone
        let has_a = c.entries.contains_key("a");
        let has_b = c.entries.contains_key("b");
        assert!(
            !has_a || !has_b,
            "expected at least one original to be evicted"
        );
    }

    #[test]
    fn hits_and_misses_count() {
        let mut c = QueryEmbedCache::new(4);
        c.insert("x".into(), vec![0.5]);
        assert!(c.get("x").is_some());
        assert!(c.get("x").is_some());
        assert!(c.get("missing").is_none());
        let (hits, misses, _) = c.stats();
        assert_eq!(hits, 2);
        assert_eq!(misses, 1);
    }

    #[test]
    fn clear_resets_state() {
        let mut c = QueryEmbedCache::new(4);
        c.insert("k".into(), vec![0.1]);
        c.clear();
        assert!(c.get("k").is_none());
        assert_eq!(c.entries.len(), 0);
    }
}

pub const SUMMARY_MODE_AUTO: &str = "auto";
pub const SUMMARY_MODE_MANUAL: &str = "manual";

#[derive(Debug, Clone, serde::Serialize)]
pub struct RuntimeGateOwner {
    pub gate: String,
    pub owner: String,
    pub detail: String,
    pub since_ms: u64,
}

#[derive(Debug, Clone)]
pub struct ActiveChatAttempt {
    pub notebook_id: String,
    pub conversation_id: String,
    pub attempt_id: String,
    pub message_id: String,
    pub cancellation: CancellationToken,
}

/// Holds an active chat registration during synchronous request preparation.
/// Dropping an unclaimed lease removes the registration, so setup/retrieval
/// failures cannot leave a conversation permanently single-flight blocked.
pub struct ActiveChatAttemptLease<'a> {
    state: &'a AppState,
    attempt: Option<ActiveChatAttempt>,
}

impl ActiveChatAttemptLease<'_> {
    pub fn cancellation(&self) -> Result<CancellationToken, GlossError> {
        self.attempt
            .as_ref()
            .map(|attempt| attempt.cancellation.clone())
            .ok_or_else(|| GlossError::Other("active chat attempt lease has no attempt".into()))
    }

    /// Transfers cleanup responsibility to the spawned stream task.
    pub fn activate(mut self) -> Result<ActiveChatAttempt, GlossError> {
        self.attempt
            .take()
            .ok_or_else(|| GlossError::Other("active chat attempt lease has no attempt".into()))
    }
}

impl Drop for ActiveChatAttemptLease<'_> {
    fn drop(&mut self) {
        if let Some(attempt) = self.attempt.take() {
            self.state.finish_active_chat_attempt(
                &attempt.notebook_id,
                &attempt.conversation_id,
                &attempt.attempt_id,
            );
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActiveStudioAttempt {
    pub notebook_id: String,
    pub attempt_id: String,
    pub cancellation: CancellationToken,
}

pub struct RuntimeGateOwnerGuard<'a> {
    state: &'a AppState,
    gate: String,
    owner: String,
    active: bool,
}

impl Drop for RuntimeGateOwnerGuard<'_> {
    fn drop(&mut self) {
        if self.active {
            self.state.clear_gate_owner(&self.gate, &self.owner);
        }
    }
}

/// Global application state managed by Tauri
pub struct AppState {
    /// App-level database (gloss.db)
    pub app_db: Mutex<AppDb>,
    /// Encrypted local secret storage for provider API keys.
    pub secret_store: SecretStore,
    /// Cached notebook database connection pools keyed by notebook ID.
    /// Each pool manages read/write connections so concurrent access to a
    /// single notebook DB is efficient (WAL mode allows concurrent readers).
    pub notebook_pools: NotebookDbPools,
    /// LLM provider registry
    pub model_registry: Mutex<ModelRegistry>,
    /// Application data directory
    pub data_dir: PathBuf,
    /// Embedding model (lazy-initialized on first use)
    pub embedder: std::sync::RwLock<Option<Arc<EmbeddingService>>>,
    /// LRU cache of (query_text, model_id) → embedding vector. The hostile
    /// audit identified that the user query is re-embedded on EVERY chat
    /// turn (80-400ms per turn) even when identical to the previous turn
    /// ("yes", "explain more", retry of a failed message). Bounded to 256
    /// entries to bound memory at ~256 * 384 * 4 = 384KB. Invalidate on
    /// embedding model change by clearing the cache (see reset below).
    pub query_embed_cache: Mutex<QueryEmbedCache>,
    /// Cached model identity that the query_embed_cache was built against.
    /// When this changes (e.g. user switches `semantic_memory_embedding_model`),
    /// the cache is flushed in `ensure_embedder`.
    pub query_embed_cache_model: Mutex<Option<String>>,
    /// Per-notebook HNSW vector indices keyed by notebook ID
    pub hnsw_indices: Mutex<HashMap<String, HnswIndex>>,
    /// Cached dims per notebook HNSW index, used by ensure_hnsw_index
    /// to detect dim mismatches when the embedding model changes (C5-FIX).
    /// We need this because HnswIndex doesn't expose its dims publicly.
    pub hnsw_index_dims: Mutex<HashMap<String, usize>>,
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
    /// Attempt-scoped active chat cancellations keyed by notebook/conversation.
    pub active_chat_attempts: Mutex<HashMap<String, ActiveChatAttempt>>,
    /// Attempt-scoped active Studio generation keyed by notebook.
    pub active_studio_attempts: Mutex<HashMap<String, ActiveStudioAttempt>>,
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
    /// Bounded replay buffer for chat transport events. The database remains
    /// the source of truth for messages; this buffer only lets the frontend
    /// recover recent missed stream/status/terminal events after listener loss.
    pub chat_stream_events: Mutex<VecDeque<crate::commands::chat::ChatStreamEventV1>>,
    pub chat_stream_next_seq: AtomicU64,
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
                    path = %redact_path(&notebook_db_path),
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
                            path = %redact_path(&notebook_db_path),
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
        Self::initialize_in_data_dir(data_dir)
    }

    /// Construct state against an explicitly selected directory. Production
    /// derives this path from ProjectDirs; tests use a temporary directory so
    /// they cannot read or mutate an operator's local Gloss state.
    fn initialize_in_data_dir(data_dir: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
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
            .get_setting("semantic_memory_auto_project")?
            .is_none()
        {
            app_db.set_setting("semantic_memory_auto_project", "true")?;
        }
        if app_db
            .get_setting("semantic_memory_turbo_quant_require_fresh_artifacts")?
            .is_none()
        {
            app_db.set_setting(
                "semantic_memory_turbo_quant_require_fresh_artifacts",
                "true",
            )?;
        }
        if app_db
            .get_setting(features::SEMANTIC_MEMORY_PROVEKV_POOL_CANDIDATES_ENABLED)?
            .is_none()
        {
            app_db.set_setting(
                features::SEMANTIC_MEMORY_PROVEKV_POOL_CANDIDATES_ENABLED,
                "false",
            )?;
        }
        if app_db
            .get_setting("semantic_memory_embedding_provider")?
            .is_none()
        {
            app_db.set_setting("semantic_memory_embedding_provider", "ollama")?;
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
            app_db.set_setting("semantic_memory_embedding_model", "bge-m3")?;
        }
        if app_db.get_setting("chunk_target_tokens")?.is_none() {
            app_db.set_setting("chunk_target_tokens", "1100")?;
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
        // The local CPU candle embedder is the automatic fallback backend.
        // Download consent defaults ON so first-time uploads just work; users
        // can switch it off in Settings → Embeddings. Existing installs that
        // still hold the old "false" default are migrated the same way.
        if app_db
            .get_setting(features::FASTEMBED_DOWNLOAD_CONSENT)?
            .map(|v| v == "false")
            .unwrap_or(true)
        {
            app_db.set_setting(features::FASTEMBED_DOWNLOAD_CONSENT, "true")?;
        }
        features::ensure_default_feature_settings(&app_db)?;
        let secret_store = SecretStore::new(&data_dir)?;
        Self::migrate_legacy_secrets(&app_db, &secret_store)?;
        Self::reconcile_notebook_source_counts(&app_db)?;

        let model_registry = ModelRegistry::new(&app_db, &secret_store)?;
        let summary_starts_paused = Self::summary_mode_starts_paused(&app_db)?;

        tracing::info!(data_dir = %redact_path(&data_dir), "Gloss initialized");

        Ok(Self {
            app_db: Mutex::new(app_db),
            secret_store,
            notebook_pools: NotebookDbPools::new(&data_dir),
            model_registry: Mutex::new(model_registry),
            data_dir,
            embedder: std::sync::RwLock::new(None),
            query_embed_cache: Mutex::new(QueryEmbedCache::default()),
            query_embed_cache_model: Mutex::new(None),
            hnsw_indices: Mutex::new(HashMap::new()),
            hnsw_index_dims: Mutex::new(HashMap::new()),
            summary_paused: AtomicBool::new(summary_starts_paused),
            ingestion_active: Arc::new(AtomicU32::new(0)),
            llm_gate: Semaphore::new(1),
            gpu_gate: Semaphore::new(1),
            gate_owners: Mutex::new(HashMap::new()),
            active_chat_attempts: Mutex::new(HashMap::new()),
            active_studio_attempts: Mutex::new(HashMap::new()),
            active_notebook_id: Mutex::new(None),
            active_epoch: AtomicU64::new(0),
            chat_grace_until: Mutex::new(0),
            last_user_activity: Mutex::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
            ),
            chat_stream_events: Mutex::new(VecDeque::new()),
            chat_stream_next_seq: AtomicU64::new(1),
        })
    }

    #[cfg(test)]
    pub(crate) fn initialize_for_test(
        data_dir: &std::path::Path,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::initialize_in_data_dir(data_dir.to_path_buf())
    }

    pub fn record_chat_stream_event(
        &self,
        attempt_id: &str,
        kind: &str,
        notebook_id: &str,
        conversation_id: &str,
        message_id: &str,
        payload: serde_json::Value,
    ) -> crate::commands::chat::ChatStreamEventV1 {
        let seq = self.chat_stream_next_seq.fetch_add(1, Ordering::SeqCst);
        let event = crate::commands::chat::ChatStreamEventV1 {
            seq,
            attempt_id: attempt_id.to_string(),
            kind: kind.to_string(),
            notebook_id: notebook_id.to_string(),
            conversation_id: conversation_id.to_string(),
            message_id: message_id.to_string(),
            payload,
            recorded_at: chrono::Utc::now().to_rfc3339(),
        };
        if let Ok(mut events) = self.chat_stream_events.lock() {
            events.push_back(event.clone());
            while events.len() > CHAT_STREAM_REPLAY_CAPACITY {
                events.pop_front();
            }
        }
        event
    }

    pub fn chat_events_since(
        &self,
        notebook_id: &str,
        conversation_id: &str,
        after_seq: Option<u64>,
    ) -> Vec<crate::commands::chat::ChatStreamEventV1> {
        let Ok(events) = self.chat_stream_events.lock() else {
            return Vec::new();
        };
        filter_chat_events_since(&events, notebook_id, conversation_id, after_seq)
    }

    /// Ensure the embedding model is initialized. Honors the configured
    /// backend from settings:
    ///
    /// - `semantic_memory_embedding_provider == "ollama"` (and reachable) →
    ///   external Ollama `/api/embed` (crash-isolated, preferred)
    /// - anything else (or an unreachable Ollama) → the in-process CPU candle
    ///   embedder (Nomic v1.5 MoE, 768d) as the automatic fallback.
    pub fn ensure_embedder(&self, app_handle: Option<&tauri::AppHandle>) -> Result<(), GlossError> {
        let mut embedder = self.embedder.write().unwrap_or_else(|e| e.into_inner());

        if embedder.is_some() {
            return Ok(());
        }

        let service = {
            let app_db = self
                .app_db
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            let provider = app_db.get_setting("semantic_memory_embedding_provider")?;
            let url = app_db.get_setting("semantic_memory_embedding_url")?;
            let model = app_db.get_setting("semantic_memory_embedding_model")?;
            let timeout_secs = app_db
                .get_setting("semantic_memory_embedding_timeout_secs")?
                .and_then(|s| s.parse::<u64>().ok());
            let download_consent = crate::commands::chat::setting_is_enabled(
                app_db.get_setting(features::FASTEMBED_DOWNLOAD_CONSENT)?,
            );
            drop(app_db);

            tracing::info!(
                provider = %provider.as_deref().unwrap_or("auto"),
                "Initializing embedding backend"
            );
            if let Some(handle) = app_handle {
                use tauri::Emitter;
                let _ = handle.emit(
                    "status:embedding_model",
                    serde_json::json!({
                        "state": "loading",
                        "message": "Loading embedding backend…"
                    }),
                );
            }
            let cache_dir = self.data_dir.join("models");
            std::fs::create_dir_all(&cache_dir)?;
            match EmbeddingService::from_configured_provider(
                provider.as_deref(),
                url.as_deref(),
                model.as_deref(),
                timeout_secs,
                &cache_dir,
                download_consent,
            ) {
                Ok(service) => service,
                Err(e) => {
                    if let Some(handle) = app_handle {
                        use tauri::Emitter;
                        let _ = handle.emit(
                            "status:embedding_model",
                            serde_json::json!({
                                "state": "error",
                                "message": format!("Embedding backend failed: {e}")
                            }),
                        );
                    }
                    return Err(e);
                }
            }
        };

        if let Some(handle) = app_handle {
            use tauri::Emitter;
            let _ = handle.emit(
                "status:embedding_model",
                serde_json::json!({
                    "state": "ready",
                    "message": "Embedding backend loaded"
                }),
            );
        }

        let provider_id = service.provider_id();
        let model_id = service.model_id().to_string();

        tracing::info!("Embedding backend ready");
        *embedder = Some(Arc::new(service));

        // Now that we know the real model identity, flush the query-embed
        // cache if it was built against a different model. This is the
        // only place the cache is invalidated on model change.
        {
            let mut cache_model = self
                .query_embed_cache_model
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            let cache_key = format!("{}:{}", provider_id, model_id);
            if cache_model.as_deref() != Some(cache_key.as_str()) {
                if let Ok(mut cache) = self.query_embed_cache.lock() {
                    cache.clear();
                }
                *cache_model = Some(cache_key.clone());
                tracing::info!(model = %cache_key, "Flushed query-embed cache on model change");
            }
        }
        Ok(())
    }

    /// Acquire a cache of the published artifact without changing durable status.
    pub fn ensure_hnsw_index(&self, notebook_id: &str) -> Result<(), GlossError> {
        let expected = {
            let guard = self
                .embedder
                .read()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            let embedder = guard
                .as_ref()
                .ok_or_else(|| GlossError::Embedding("Embedder not initialized".into()))?;
            EmbeddingIndexMetadata::ready(
                NATIVE_HNSW_INDEX_ID,
                embedder.provider_id(),
                embedder.model_id(),
                embedder.model_digest(),
                embedder.dims(),
            )
        };
        let db_path = self.notebook_db_path(notebook_id)?;
        let index_path = crate::ingestion::embed::native_dense_artifact_path(
            db_path
                .parent()
                .ok_or_else(|| GlossError::Other("Notebook DB has no parent".into()))?,
        );
        let mut indices = self
            .hnsw_indices
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let stored = self.with_notebook_db(notebook_id, |db| {
            db.embedding_index_metadata(NATIVE_HNSW_INDEX_ID)
        })?;
        if !stored
            .as_ref()
            .is_some_and(|stored| stored.identity_matches(&expected))
        {
            indices.remove(notebook_id);
            self.hnsw_index_dims
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?
                .remove(notebook_id);
            return Err(GlossError::Embedding(
                "Native dense index is not ready for the configured embedding identity".into(),
            ));
        }
        if let Some(index) = indices.get(notebook_id) {
            if !index.has_pending_changes() && index.is_current(&index_path)? {
                return Ok(());
            }
        }
        let index = self.with_notebook_db(notebook_id, |db| {
            crate::ingestion::dense::load_published_dense_index(db, &index_path, &expected)
        })?;
        indices.insert(notebook_id.to_string(), index);
        self.hnsw_index_dims
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?
            .insert(notebook_id.to_string(), expected.dimensions.unwrap_or(0));
        Ok(())
    }

    /// Persist detached-vector cleanup without promoting publication metadata.
    pub fn save_hnsw_index(&self, notebook_id: &str) -> Result<(), GlossError> {
        let db_path = self.notebook_db_path(notebook_id)?;
        let index_path = crate::ingestion::embed::native_dense_artifact_path(
            db_path
                .parent()
                .ok_or_else(|| GlossError::Other("Notebook DB has no parent".into()))?,
        );
        let indices = self
            .hnsw_indices
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        if let Some(index) = indices.get(notebook_id) {
            if index.has_pending_changes() {
                self.with_notebook_db_write(notebook_id, |db| {
                    crate::ingestion::dense::publish_dense_cleanup(db, &index_path, index)
                })?;
            }
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

    pub fn gate_owner_guard<'a>(
        &'a self,
        gate: &str,
        owner: &str,
        detail: &str,
    ) -> RuntimeGateOwnerGuard<'a> {
        self.set_gate_owner(gate, owner, detail);
        RuntimeGateOwnerGuard {
            state: self,
            gate: gate.to_string(),
            owner: owner.to_string(),
            active: true,
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

    fn active_chat_key(notebook_id: &str, conversation_id: &str) -> String {
        format!("{notebook_id}:{conversation_id}")
    }

    pub fn register_active_chat_attempt(
        &self,
        notebook_id: &str,
        conversation_id: &str,
        attempt_id: &str,
        message_id: &str,
    ) -> Result<ActiveChatAttempt, ActiveChatAttempt> {
        let key = Self::active_chat_key(notebook_id, conversation_id);
        let mut active = self
            .active_chat_attempts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = active.get(&key) {
            if !existing.cancellation.is_cancelled() {
                return Err(existing.clone());
            }
        }

        let attempt = ActiveChatAttempt {
            notebook_id: notebook_id.to_string(),
            conversation_id: conversation_id.to_string(),
            attempt_id: attempt_id.to_string(),
            message_id: message_id.to_string(),
            cancellation: CancellationToken::new(),
        };
        active.insert(key, attempt.clone());
        Ok(attempt)
    }

    pub fn lease_active_chat_attempt(
        &self,
        notebook_id: &str,
        conversation_id: &str,
        attempt_id: &str,
        message_id: &str,
    ) -> Result<ActiveChatAttemptLease<'_>, ActiveChatAttempt> {
        self.register_active_chat_attempt(notebook_id, conversation_id, attempt_id, message_id)
            .map(|attempt| ActiveChatAttemptLease {
                state: self,
                attempt: Some(attempt),
            })
    }

    pub fn finish_active_chat_attempt(
        &self,
        notebook_id: &str,
        conversation_id: &str,
        attempt_id: &str,
    ) {
        let key = Self::active_chat_key(notebook_id, conversation_id);
        let mut active = self
            .active_chat_attempts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if active
            .get(&key)
            .map(|attempt| attempt.attempt_id == attempt_id)
            .unwrap_or(false)
        {
            active.remove(&key);
        }
    }

    pub fn cancel_active_chats_for_notebook(&self, notebook_id: &str) -> Vec<ActiveChatAttempt> {
        let active = self
            .active_chat_attempts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        active
            .values()
            .filter(|attempt| attempt.notebook_id == notebook_id)
            .cloned()
            .inspect(|attempt| attempt.cancellation.cancel())
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
        let pool = self.notebook_pools.get_or_create(notebook_id, || {
            let app_db = self
                .app_db
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            let notebook = app_db.get_notebook(notebook_id)?;
            Ok(PathBuf::from(notebook.directory).join("notebook.db"))
        })?;
        Ok(pool.db_path().to_path_buf())
    }

    /// Execute a function with a notebook database connection.
    /// Uses the connection pool: read operations use pooled read connections
    /// (concurrent readers via WAL mode), write operations use the single
    /// exclusive write connection.
    pub fn with_notebook_db<F, T>(&self, notebook_id: &str, f: F) -> Result<T, GlossError>
    where
        F: FnOnce(&NotebookDb) -> Result<T, GlossError>,
    {
        let pool = self.notebook_pools.get_or_create(notebook_id, || {
            let app_db = self
                .app_db
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            let notebook = app_db.get_notebook(notebook_id)?;
            Ok(PathBuf::from(notebook.directory).join("notebook.db"))
        })?;
        pool.read(f)
    }

    /// Execute a write operation against the notebook database.
    #[allow(dead_code)]
    /// Uses the pool's exclusive write connection (only one writer at a time).
    pub fn with_notebook_db_write<F, T>(&self, notebook_id: &str, f: F) -> Result<T, GlossError>
    where
        F: FnOnce(&NotebookDb) -> Result<T, GlossError>,
    {
        let pool = self.notebook_pools.get_or_create(notebook_id, || {
            let app_db = self
                .app_db
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            let notebook = app_db.get_notebook(notebook_id)?;
            Ok(PathBuf::from(notebook.directory).join("notebook.db"))
        })?;
        pool.write(f)
    }

    /// Get a cached embedding for the query, or embed it fresh and store it.
    /// This is the LRU cache wrapper used by the chat hot path to avoid
    /// re-embedding identical follow-up questions ("yes", "explain more",
    /// retry of a failed message) on every chat turn.
    ///
    /// Returns the embedding vector; the caller is responsible for the HNSW
    /// search and chunk lookup. On embed error, returns the error unchanged
    /// (the caller decides whether to silently degrade to BM25 or surface
    /// a toast).
    pub fn get_or_embed_query(&self, query: &str) -> Result<Vec<f32>, GlossError> {
        // Cache lookup
        {
            let mut cache = self
                .query_embed_cache
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            if let Some(vec) = cache.get(query) {
                return Ok(vec);
            }
        }

        // Cache miss: embed fresh
        let embedding = {
            let embedder_arc: Arc<EmbeddingService> = {
                let guard = self.embedder.read().unwrap_or_else(|e| e.into_inner());
                guard
                    .as_ref()
                    .ok_or_else(|| GlossError::Embedding("Embedder not initialized".into()))?
                    .clone()
            };
            embedder_arc.embed_one(query)?
        };

        // Store
        if let Ok(mut cache) = self.query_embed_cache.lock() {
            cache.insert(query.to_string(), embedding.clone());
        }
        Ok(embedding)
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
        let dense_block_reason = self.with_notebook_db(notebook_id, |nb_db| {
            let metadata = nb_db.embedding_index_metadata(NATIVE_HNSW_INDEX_ID)?;
            Ok(match metadata.as_ref().map(|m| m.status.as_str()) {
                Some("ready") => None,
                Some("stale") | Some("blocked") => {
                    Some(RetrievalReasonCode::EmbeddingIndexMetadataStale)
                }
                Some(_) | None => Some(RetrievalReasonCode::EmbeddingIndexMetadataUnknown),
            })
        })?;
        self.with_notebook_db(notebook_id, |nb_db| {
            // TODO(B1-followup): move this blocking call to spawn_blocking in the
            // commands/chat caller once we validate perf gains.
            let embedder = {
                let embedder_guard = self.embedder.read().unwrap_or_else(|e| e.into_inner());
                embedder_guard.clone()
            };
            let indices_guard = self.hnsw_indices.lock().unwrap_or_else(|e| e.into_inner());
            let index = indices_guard.get(notebook_id);
            // Pre-compute the query embedding through the LRU cache so that
            // repeated identical queries ("yes", "explain more", a regenerated
            // message) don't re-hit Ollama / FastEmbed. The cache miss path
            // (which actually calls embed_one) is what gets the timeout / error
            // surfaced to the user. (See hostile audit B4.)
            let cached_query_embedding = match embedder.as_deref() {
                Some(_emb) => match self.get_or_embed_query(query) {
                    Ok(vec) => Some(vec),
                    Err(e) => {
                        tracing::warn!(error = %e, "query embedding failed; dense retrieval will be skipped");
                        None
                    }
                },
                None => None,
            };
            hybrid_search::local_retrieval_outcome_with_query(
                query,
                nb_db,
                embedder.as_deref(),
                index,
                cached_query_embedding.as_deref(),
                dense_block_reason.clone(),
                NATIVE_SEMANTIC_INDEXING_ENABLED,
                scope,
                top_k,
                trace_ref,
            )
        })
    }
}

fn filter_chat_events_since(
    events: &VecDeque<crate::commands::chat::ChatStreamEventV1>,
    notebook_id: &str,
    conversation_id: &str,
    after_seq: Option<u64>,
) -> Vec<crate::commands::chat::ChatStreamEventV1> {
    let after_seq = after_seq.unwrap_or(0);
    events
        .iter()
        .filter(|event| {
            event.seq > after_seq
                && event.notebook_id == notebook_id
                && event.conversation_id == conversation_id
        })
        .cloned()
        .collect()
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
    fn test_state_initializes_in_supplied_temp_dir() {
        let dir = tempdir().unwrap();
        let state = AppState::initialize_for_test(dir.path()).expect("test state initializes");

        assert_eq!(state.data_dir, dir.path());
        assert!(dir.path().join("gloss.db").is_file());
        assert!(dir.path().join("notebooks").is_dir());
    }

    /// Reproduces the production call pattern for `ensure_embedder` (warmup
    /// spawn, chat command, import path all call it from inside a tokio async
    /// runtime) with the Ollama provider configured — exactly the user setup
    /// that has been hitting "poisoned lock" cascades in the field. If the
    /// blocking HTTP probe panics inside the async context, this test captures
    /// the panic message so the original trigger is visible.
    ///
    /// Run with:
    /// `cargo test --features semantic-memory-turbo-quant state::tests::ollama_embedder_initializes_inside_async_context -- --ignored --nocapture`
    #[test]
    #[ignore] // requires a reachable Ollama on localhost:11434
    fn ollama_embedder_initializes_inside_async_context() {
        let dir = tempdir().unwrap();
        let state = AppState::initialize_for_test(dir.path()).expect("test state initializes");
        {
            let app_db = state.app_db.lock().unwrap_or_else(|e| e.into_inner());
            app_db
                .set_setting("semantic_memory_embedding_provider", "ollama")
                .unwrap();
            app_db
                .set_setting("semantic_memory_embedding_url", "http://localhost:11434")
                .unwrap();
        }

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async {
                state.ensure_embedder(None).expect("embedder initializes");
            });
        }));

        match result {
            Ok(()) => {
                let guard = state.embedder.read().unwrap_or_else(|e| e.into_inner());
                let svc = guard.as_ref().expect("embedder stored");
                println!(
                    "ok: embedder {} ({}) dims={}",
                    svc.provider_id(),
                    svc.model_id(),
                    svc.dims()
                );
            }
            Err(payload) => {
                let msg = payload
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "<non-string panic>".to_string());
                panic!("PANIC INSIDE ASYNC ensure_embedder: {msg}");
            }
        }
    }

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
                    processing_state: None,
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
            .conn()
            .execute("ALTER TABLE providers ADD COLUMN api_key TEXT", [])
            .unwrap();
        app_db
            .conn()
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

    #[test]
    fn chat_replay_buffer_returns_ordered_events_after_listener_gap() {
        let mut events = VecDeque::new();
        for (seq, conversation_id, kind) in [
            (1, "conv-1", "status"),
            (2, "conv-2", "token"),
            (3, "conv-1", "token"),
            (4, "conv-1", "done"),
        ] {
            events.push_back(crate::commands::chat::ChatStreamEventV1 {
                seq,
                attempt_id: "attempt-1".to_string(),
                kind: kind.to_string(),
                notebook_id: "nb-1".to_string(),
                conversation_id: conversation_id.to_string(),
                message_id: "msg-1".to_string(),
                payload: serde_json::json!({ "kind": kind }),
                recorded_at: "2026-06-12T00:00:00Z".to_string(),
            });
        }

        let replay = filter_chat_events_since(&events, "nb-1", "conv-1", Some(1));
        assert_eq!(
            replay
                .iter()
                .map(|event| (event.seq, event.kind.as_str()))
                .collect::<Vec<_>>(),
            vec![(3, "token"), (4, "done")]
        );
    }
}
