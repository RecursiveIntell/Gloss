#![allow(deprecated)]

//! # semantic-memory
//!
//! Local-first semantic memory backed by authoritative SQLite state and an optional recoverable
//! HNSW sidecar.
//!
//! The crate stores facts, chunked documents, conversation messages, and searchable episodes in
//! SQLite. Search combines BM25 (FTS5) and vector retrieval with Reciprocal Rank Fusion, and
//! `search_explained()` returns the exact scoring breakdown from the live pipeline.
//!
//! Concurrency uses one writer connection plus a pool of WAL-enabled reader connections.
//! Durable writes are committed to SQLite first; any required HNSW sidecar mutations are journaled
//! in SQLite and replayed on open, flush, rebuild, or reconcile.
//!
//! `search()` targets facts, document chunks, and episodes by default. Message retrieval is
//! available through `search_conversations()` or by opting into
//! [`SearchSourceType::Messages`](crate::SearchSourceType::Messages).
//!
//! Integrity tooling is strict about malformed stored data: invalid roles, JSON, enums, embedding
//! blobs, quantized blobs, and sidecar drift are surfaced through `verify_integrity()` instead of
//! being silently converted into defaults. `reconcile()` can rebuild FTS or fully re-embed and
//! rebuild derived state from SQLite.
//!
//! `store.graph_view()` exposes a deterministic graph traversal layer over namespaces, facts,
//! documents, chunks, sessions, messages, episodes, and semantic/temporal/causal links derived
//! from SQLite state.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use semantic_memory::{MemoryConfig, MemoryStore};
//!
//! # async fn example() -> Result<(), semantic_memory::MemoryError> {
//! let store = MemoryStore::open(MemoryConfig::default())?;
//!
//! // Store a fact
//! store.add_fact("general", "Rust was first released in 2015", None, None).await?;
//!
//! // Search
//! let results = store.search("when was Rust released", None, None, None).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Operational Notes
//!
//! - SQLite is authoritative for all durable records and embeddings.
//! - HNSW is an acceleration sidecar. Pending sidecar mutations are journaled in SQLite, so a
//!   sidecar failure does not imply the SQLite write rolled back.
//! - WAL mode plus pooled reader connections allows concurrent reads while writes serialize through
//!   the writer connection.
//! - `search_explained()` reflects the exact ranking math used by the active search pipeline,
//!   including reranking from exact f32 cosine similarity when configured.

// At least one search backend must be enabled.
#[cfg(not(any(feature = "hnsw", feature = "brute-force")))]
compile_error!("At least one search backend feature must be enabled: 'hnsw' or 'brute-force'");

pub mod chunker;
pub mod config;
pub(crate) mod conversation;
pub mod db;
pub(crate) mod documents;
pub mod embedder;
pub(crate) mod episodes;
pub mod error;
mod graph;
#[cfg(feature = "hnsw")]
pub mod hnsw;
#[cfg(feature = "hnsw")]
mod hnsw_ops;
mod json_compat_import;
pub(crate) mod knowledge;
mod pool;
mod projection_batch;
mod projection_derivation;
/// Compatibility-only legacy import surface.
///
/// This module exists only for migration compatibility with pre-V11 import paths.
#[deprecated(
    since = "0.6.0",
    note = "Legacy V10 import path is migration-only. Use `import_projection_batch()` with `ProjectionImportBatchV3` on the canonical lane."
)]
#[doc(hidden)]
pub mod projection_import;
mod projection_lane;
mod projection_legacy_compat;
pub(crate) mod projection_storage;
pub mod quantize;
pub mod search;
pub mod storage;
mod store_support;
pub mod tokenizer;
pub mod types;
pub mod vector_codec;

// Re-export primary public types.
pub use config::{
    ChunkingConfig, DerivedVectorBackendPolicy, EmbeddingConfig, MemoryConfig, MemoryLimits,
    PoolConfig, SearchConfig,
};
pub use db::{IntegrityReport, ReconcileAction, VerifyMode};
pub use embedder::{Embedder, MockEmbedder, OllamaEmbedder};
pub use error::MemoryError;
#[cfg(feature = "hnsw")]
pub use hnsw::{HnswConfig, HnswHit, HnswIndex};
pub(crate) use projection_lane::projection_import_failure_id;
pub use projection_lane::{
    ProjectionImportFailureReceiptEntry, ProjectionImportLogEntry, ProjectionImportResult,
};
pub use quantize::{pack_quantized, unpack_quantized, QuantizedVector, Quantizer};
pub use storage::StoragePaths;
pub use tokenizer::{EstimateTokenCounter, TokenCounter};
pub use types::{
    ChunkManifestChunkMapping, ChunkManifestEntry, ChunkManifestIngestOptions,
    ChunkManifestIngestResult, Document, EmbeddingDisplacement, EpisodeMeta, EpisodeOutcome,
    ExactnessProfile, ExplainedResult, ExplainedResultAnswerV1, ExplainedSearchResponse, Fact,
    GraphDirection, GraphEdge, GraphEdgeType, GraphView, MemoryStats, Message,
    NamespaceDeleteReport, ProjectionClaimVersion, ProjectionEntityAlias, ProjectionEpisode,
    ProjectionEvidenceRef, ProjectionQuery, ProjectionRelationVersion, ReceiptMode, Role,
    ScoreBreakdown, SearchContext, SearchReceiptAnswersV1, SearchReplayReportV1, SearchResponse,
    SearchResult, SearchSource, SearchSourceType, Session, TextChunk, VectorArtifactBuildReceiptV1,
    VectorSearchReceiptV1, VerificationStatus,
};
#[cfg(feature = "turbo-quant-codec")]
pub use vector_codec::TurboQuantCodec;
pub use vector_codec::{
    RawF32Codec, Sq8Codec, VectorArtifactV1, VectorCodec, VectorCodecProfileV1,
};

use std::sync::Arc;

const MAX_TOP_K: usize = 1_000;
#[cfg(feature = "hnsw")]
const MAX_HNSW_CANDIDATES: usize = 10_000;

pub(crate) use store_support::{
    as_str_slice, build_episode_search_text, merge_trace_ctx, to_owned_string_vec,
    verification_status_for_outcome,
};

#[cfg(feature = "hnsw")]
fn verify_hnsw_key_level_integrity(
    conn: &rusqlite::Connection,
    dimensions: usize,
    node_vectors: &std::collections::HashMap<usize, Vec<f32>>,
    sidecar_files_exist: bool,
) -> Result<Vec<String>, MemoryError> {
    let mut issues = Vec::new();
    let mut live_rows: std::collections::HashMap<String, Vec<f32>> =
        std::collections::HashMap::new();

    let mut live_stmt = conn.prepare(
        "SELECT 'fact:' || id, embedding FROM facts WHERE embedding IS NOT NULL
         UNION ALL
         SELECT 'chunk:' || id, embedding FROM chunks WHERE embedding IS NOT NULL
         UNION ALL
         SELECT 'msg:' || id, embedding FROM messages WHERE embedding IS NOT NULL
         UNION ALL
         SELECT 'episode:' || episode_id, embedding FROM episodes WHERE embedding IS NOT NULL",
    )?;
    let live_iter = live_stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
    })?;
    for row in live_iter {
        let (key, blob) = row?;
        match db::decode_f32_le(&blob, dimensions) {
            Ok(vector) => {
                live_rows.insert(key, vector);
            }
            Err(err) => issues.push(format!(
                "HNSW live embedding row {key} has invalid vector: {err}"
            )),
        }
    }

    if !live_rows.is_empty() && !sidecar_files_exist {
        issues.push(format!(
            "HNSW sidecar files are missing while {} embedded rows exist in SQLite",
            live_rows.len()
        ));
    }

    let keymap_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='hnsw_keymap'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !keymap_exists {
        if !live_rows.is_empty() {
            issues.push("HNSW keymap table missing while embedded SQLite rows exist".to_string());
        }
        return Ok(issues);
    }

    let mut active_keymap: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut keymap_stmt =
        conn.prepare("SELECT node_id, item_key FROM hnsw_keymap WHERE deleted = 0")?;
    let keymap_iter = keymap_stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in keymap_iter {
        let (node_id_raw, key) = row?;
        let Some((domain, raw_id)) = key.split_once(':') else {
            issues.push(format!("HNSW keymap entry has malformed key: {key}"));
            continue;
        };
        if !matches!(domain, "fact" | "chunk" | "msg" | "episode") || raw_id.is_empty() {
            issues.push(format!(
                "HNSW keymap entry has unsupported key domain: {key}"
            ));
            continue;
        }
        if domain == "msg" && raw_id.parse::<i64>().is_err() {
            issues.push(format!("HNSW message key has non-integer row id: {key}"));
            continue;
        }
        let node_id = match usize::try_from(node_id_raw) {
            Ok(node_id) => node_id,
            Err(err) => {
                issues.push(format!(
                    "HNSW keymap node_id {node_id_raw} is invalid: {err}"
                ));
                continue;
            }
        };
        active_keymap.insert(key, node_id);
    }

    for key in live_rows.keys() {
        if !active_keymap.contains_key(key) {
            issues.push(format!(
                "HNSW keymap missing live embedded SQLite row: {key}"
            ));
        }
    }

    for (key, node_id) in &active_keymap {
        let Some(live_vector) = live_rows.get(key) else {
            issues.push(format!(
                "HNSW keymap has stale active entry without live embedded SQLite row: {key}"
            ));
            continue;
        };
        let Some(index_vector) = node_vectors.get(node_id) else {
            issues.push(format!(
                "HNSW keymap entry {key} points to missing in-memory node vector {node_id}"
            ));
            continue;
        };
        if index_vector.len() != live_vector.len()
            || index_vector
                .iter()
                .zip(live_vector)
                .any(|(left, right)| left.to_bits() != right.to_bits())
        {
            issues.push(format!(
                "HNSW keymap entry {key} points to node {node_id} whose vector does not match the authoritative SQLite embedding"
            ));
        }
    }

    if active_keymap.len() != live_rows.len() {
        issues.push(format!(
            "HNSW keymap drift: {} active keymap rows vs {} embedded SQLite rows",
            active_keymap.len(),
            live_rows.len()
        ));
    }

    Ok(issues)
}

/// Compatibility-only public access to retained legacy surfaces.
#[doc(hidden)]
pub mod compat {
    #[deprecated(
        since = "0.5.0",
        note = "Legacy ImportEnvelope is migration-only. New integrations should use `ProjectionImportBatchV3` on the canonical lane."
    )]
    #[doc(hidden)]
    #[allow(deprecated)]
    pub mod legacy_import_envelope {
        pub use crate::projection_import::{
            ImportEnvelope, ImportProjectionFreshness, ImportReceipt, ImportRecord, ImportStatus,
        };
        pub use stack_ids::EnvelopeId;
    }

    #[deprecated(
        since = "0.5.0",
        note = "Legacy trace_id is migration-only. Use `stack_ids::TraceCtx`."
    )]
    #[doc(hidden)]
    #[allow(deprecated)]
    pub mod compat_trace_id {
        pub use crate::types::TraceId;
    }
}

/// Thread-safe handle to the memory database.
///
/// Clone is cheap (Arc internals). `Send + Sync`.
#[derive(Clone)]
pub struct MemoryStore {
    inner: Arc<MemoryStoreInner>,
}

struct MemoryStoreInner {
    pool: pool::SqlitePool,
    embedder: Box<dyn Embedder>,
    embedding_permits: Arc<tokio::sync::Semaphore>,
    config: MemoryConfig,
    paths: StoragePaths,
    token_counter: Arc<dyn TokenCounter>,
    #[cfg(feature = "hnsw")]
    hnsw_index: std::sync::RwLock<HnswIndex>,
}

#[cfg(feature = "hnsw")]
impl Drop for MemoryStoreInner {
    fn drop(&mut self) {
        if !self.paths.hnsw_dir.exists() {
            tracing::debug!(
                path = %self.paths.hnsw_dir.display(),
                "Skipping HNSW drop flush because the sidecar directory no longer exists"
            );
            return;
        }

        let pending_ops = match self.pool.with_read_conn(db::pending_index_op_count) {
            Ok(count) => count,
            Err(err) => {
                tracing::warn!("Failed to inspect pending HNSW work on drop: {}", err);
                0
            }
        };

        if pending_ops > 0 {
            if let Err(err) =
                hnsw_ops::recover_hnsw_sidecar_sync(&self.pool, &self.paths, &self.config.hnsw)
            {
                tracing::error!("Failed to recover and flush HNSW on drop: {}", err);
            }
            return;
        }

        let hnsw_guard = match self.hnsw_index.read() {
            Ok(g) => g,
            Err(_) => {
                tracing::warn!("HNSW RwLock poisoned on drop — skipping save");
                return;
            }
        };

        if let Err(err) = hnsw_ops::save_hnsw_sidecar(
            &hnsw_guard,
            &self.paths.hnsw_dir,
            &self.paths.hnsw_basename,
        ) {
            tracing::error!("Failed to save HNSW index on drop: {}", err);
        }

        // Flush key mappings to SQLite
        if let Err(e) = self
            .pool
            .with_write_conn(|conn| hnsw_guard.flush_keymap(conn))
        {
            tracing::error!("Failed to flush HNSW keymap on drop: {}", e);
        }
    }
}

impl MemoryStore {
    /// Run read-only work on a pooled reader connection on a blocking thread.
    ///
    /// This prevents SQLite I/O from stalling the tokio executor while allowing
    /// multiple concurrent readers under WAL mode.
    async fn with_read_conn<F, T>(&self, f: F) -> Result<T, MemoryError>
    where
        F: FnOnce(&rusqlite::Connection) -> Result<T, MemoryError> + Send + 'static,
        T: Send + 'static,
    {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<T, MemoryError> {
            inner.pool.with_read_conn(f)
        })
        .await
        .map_err(|e| MemoryError::Other(format!("Blocking task panicked: {}", e)))?
    }

    /// Run write-capable work on the single writer connection on a blocking thread.
    async fn with_write_conn<F, T>(&self, f: F) -> Result<T, MemoryError>
    where
        F: FnOnce(&rusqlite::Connection) -> Result<T, MemoryError> + Send + 'static,
        T: Send + 'static,
    {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<T, MemoryError> {
            inner.pool.with_write_conn(f)
        })
        .await
        .map_err(|e| MemoryError::Other(format!("Blocking task panicked: {}", e)))?
    }

    async fn persist_search_receipt(
        &self,
        receipt: &VectorSearchReceiptV1,
    ) -> Result<(), MemoryError> {
        let receipt = receipt.clone();
        self.with_write_conn(move |conn| db::store_search_receipt(conn, &receipt))
            .await
    }

    /// Run HNSW search on a blocking thread to avoid holding std::sync::RwLock
    /// across await points (CONC-001).
    #[cfg(feature = "hnsw")]
    async fn hnsw_search_blocking(
        &self,
        query_embedding: Vec<f32>,
        candidates: usize,
    ) -> Vec<HnswHit> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let guard = inner.hnsw_index.read().unwrap_or_else(|e| e.into_inner());
            match guard.search(&query_embedding, candidates) {
                Ok(hits) => hits,
                Err(e) => {
                    tracing::error!(
                        "HNSW search failed, falling back to brute-force vector search: {}",
                        e
                    );
                    Vec::new()
                }
            }
        })
        .await
        .unwrap_or_else(|e| {
            tracing::error!("HNSW search blocking task panicked: {}", e);
            Vec::new()
        })
    }

    #[cfg(feature = "hnsw")]
    fn sync_pending_hnsw_ops_blocking(&self) -> Result<usize, MemoryError> {
        hnsw_ops::sync_pending_hnsw_sidecar(&self.inner)
    }

    #[cfg(feature = "hnsw")]
    async fn sync_pending_hnsw_ops(&self) -> Result<usize, MemoryError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || hnsw_ops::sync_pending_hnsw_sidecar(&inner))
            .await
            .map_err(|e| MemoryError::Other(format!("Blocking task panicked: {}", e)))?
    }

    #[cfg(feature = "hnsw")]
    async fn sync_pending_hnsw_ops_best_effort(&self, operation: &'static str) {
        if let Err(err) = self.sync_pending_hnsw_ops().await {
            tracing::warn!(
                operation,
                error = %err,
                "SQLite write committed but HNSW sidecar sync is still pending"
            );
        } else {
            self.maybe_flush_hnsw();
        }
    }

    /// Open or create a memory store at the configured base directory.
    ///
    /// Creates the directory if it doesn't exist, opens/creates SQLite,
    /// runs migrations, and initializes the HNSW index.
    pub fn open(config: MemoryConfig) -> Result<Self, MemoryError> {
        let config = config.normalize_and_validate()?;
        let embedder = Box::new(OllamaEmbedder::try_new(&config.embedding)?);
        Self::open_with_embedder(config, embedder)
    }

    /// Open with a custom embedder (for testing or non-Ollama providers).
    #[allow(unused_mut)] // `config` is mutated only when the `hnsw` feature is enabled
    pub fn open_with_embedder(
        mut config: MemoryConfig,
        embedder: Box<dyn Embedder>,
    ) -> Result<Self, MemoryError> {
        config = config.normalize_and_validate()?;
        if embedder.dimensions() != config.embedding.dimensions {
            return Err(MemoryError::DimensionMismatch {
                expected: config.embedding.dimensions,
                actual: embedder.dimensions(),
            });
        }
        config.embedding.model = embedder.model_name().to_string();

        let paths = StoragePaths::new(&config.base_dir);

        // Create directory if needed
        std::fs::create_dir_all(&paths.base_dir).map_err(|e| {
            MemoryError::StorageError(format!(
                "Failed to create directory {}: {}",
                paths.base_dir.display(),
                e
            ))
        })?;

        let pool = pool::SqlitePool::open(&paths.sqlite_path, &config.pool, &config.limits)?;
        pool.with_write_conn(|conn| db::check_embedding_metadata(conn, &config.embedding))?;

        // Ensure HNSW dimensions match the embedding config
        #[cfg(feature = "hnsw")]
        {
            config.hnsw.dimensions = config.embedding.dimensions;
        }

        let token_counter = config
            .token_counter
            .clone()
            .unwrap_or_else(tokenizer::default_token_counter);

        #[cfg(feature = "hnsw")]
        let hnsw_index = {
            let hnsw_config = config.hnsw.clone();

            let embeddings_dirty = pool.with_read_conn(db::is_embeddings_dirty)?;
            let pending_index_ops = pool.with_read_conn(db::pending_index_op_count)?;

            if embeddings_dirty {
                // Embedding model changed — old HNSW index is useless.
                // Create a fresh index; reembed_all() will rebuild it.
                tracing::warn!(
                    "Embedding model changed — creating fresh HNSW index (old index is stale)"
                );
                pool.with_write_conn(|conn| {
                    db::clear_all_pending_index_ops(conn)?;
                    db::set_sidecar_dirty(conn, false)?;
                    Ok(())
                })?;
                HnswIndex::new(hnsw_config)?
            } else if pending_index_ops > 0 || pool.with_read_conn(db::is_sidecar_dirty)? {
                tracing::warn!(
                    pending_index_ops,
                    "Recovering HNSW sidecar from SQLite because durable sidecar work exists"
                );
                hnsw_ops::recover_hnsw_sidecar_sync(&pool, &paths, &hnsw_config)?
            } else if paths.hnsw_files_exist() {
                tracing::info!("Loading HNSW index from {:?}", paths.hnsw_dir);
                match HnswIndex::load(&paths.hnsw_dir, &paths.hnsw_basename, hnsw_config.clone()) {
                    Ok(index) => {
                        // Load key mappings from SQLite
                        if let Err(e) = pool.with_write_conn(|conn| index.load_keymap(conn)) {
                            tracing::warn!("Failed to load HNSW key mappings: {}. Mappings will be empty until rebuild.", e);
                        }

                        // Stale index detection: compare HNSW entry count vs SQLite
                        // embedding count. A mismatch means the app crashed before
                        // flushing HNSW, or keys were lost.
                        let hnsw_count = index.len();
                        let sqlite_count: i64 = pool.with_read_conn(|conn| {
                            Ok(conn.query_row(
                                    "SELECT (SELECT COUNT(*) FROM facts WHERE embedding IS NOT NULL) +
                                        (SELECT COUNT(*) FROM chunks WHERE embedding IS NOT NULL) +
                                        (SELECT COUNT(*) FROM messages WHERE embedding IS NOT NULL) +
                                        (SELECT COUNT(*) FROM episodes WHERE embedding IS NOT NULL)",
                                    [],
                                    |row| row.get(0),
                                )?)
                        })?;

                        let drift = (sqlite_count - hnsw_count as i64).abs();
                        if drift > 0 {
                            tracing::warn!(
                                hnsw_count,
                                sqlite_count,
                                drift,
                                "HNSW index is stale — {} entries differ from SQLite. \
                                 Likely caused by unclean shutdown. Triggering inline rebuild.",
                                drift
                            );
                            // Discard the stale index and rebuild from SQLite
                            let rebuilt =
                                hnsw_ops::recover_hnsw_sidecar_sync(&pool, &paths, &hnsw_config)?;
                            tracing::info!(
                                active = rebuilt.len(),
                                "HNSW index rebuilt after stale detection"
                            );
                            rebuilt
                        } else {
                            tracing::info!(
                                "HNSW index loaded ({} active keys, in sync with SQLite)",
                                hnsw_count
                            );
                            index
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to load HNSW index: {}. Rebuilding sidecar from authoritative SQLite rows.",
                            e
                        );
                        hnsw_ops::recover_hnsw_sidecar_sync(&pool, &paths, &hnsw_config)?
                    }
                }
            } else {
                // Check if SQLite has embeddings that should be in the index.
                // This happens when: sidecar files were deleted, data dir was
                // partially copied, app crashed before first flush, or HNSW was
                // added after data already existed.
                let orphan_count: i64 = pool.with_read_conn(|conn| {
                    Ok(conn.query_row(
                        "SELECT (SELECT COUNT(*) FROM facts WHERE embedding IS NOT NULL) +
                                (SELECT COUNT(*) FROM chunks WHERE embedding IS NOT NULL) +
                                (SELECT COUNT(*) FROM messages WHERE embedding IS NOT NULL) +
                                (SELECT COUNT(*) FROM episodes WHERE embedding IS NOT NULL)",
                        [],
                        |row| row.get(0),
                    )?)
                })?;

                if orphan_count > 0 {
                    tracing::warn!(
                        orphan_count,
                        "HNSW sidecar files missing but {} embeddings exist in SQLite — \
                         rebuilding index inline",
                        orphan_count
                    );
                    let new_index =
                        hnsw_ops::recover_hnsw_sidecar_sync(&pool, &paths, &hnsw_config)?;
                    tracing::info!(
                        active = new_index.len(),
                        "HNSW index rebuilt from SQLite embeddings"
                    );
                    new_index
                } else {
                    tracing::info!("Creating new empty HNSW index (no embeddings in SQLite)");
                    HnswIndex::new(hnsw_config)?
                }
            }
        };

        let store = Self {
            inner: Arc::new(MemoryStoreInner {
                pool,
                embedder,
                embedding_permits: Arc::new(tokio::sync::Semaphore::new(
                    config.limits.max_embedding_concurrency,
                )),
                config,
                paths,
                token_counter,
                #[cfg(feature = "hnsw")]
                hnsw_index: std::sync::RwLock::new(hnsw_index),
            }),
        };

        #[cfg(feature = "hnsw")]
        if let Err(err) = store.sync_pending_hnsw_ops_blocking() {
            tracing::warn!(
                error = %err,
                "Failed to reconcile pending HNSW sidecar ops during open; sidecar replay remains pending"
            );
        }

        Ok(store)
    }

    async fn with_embedding_permit(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, MemoryError> {
        self.inner
            .embedding_permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| MemoryError::Other(format!("embedding semaphore closed: {e}")))
    }

    async fn embed_text_internal(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
        let _permit = self.with_embedding_permit().await?;
        let embedding = self.inner.embedder.embed(text).await?;
        db::validate_embedding(&embedding, self.inner.config.embedding.dimensions)?;
        Ok(embedding)
    }

    async fn embed_batch_internal(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, MemoryError> {
        let requested = texts.len();
        let _permit = self.with_embedding_permit().await?;
        let embeddings = self.inner.embedder.embed_batch(texts).await?;
        db::validate_embedding_batch(
            &embeddings,
            requested,
            self.inner.config.embedding.dimensions,
        )?;
        Ok(embeddings)
    }

    fn validate_embedding_dimensions(&self, embedding: &[f32]) -> Result<(), MemoryError> {
        db::validate_embedding(embedding, self.inner.config.embedding.dimensions)
    }

    fn validate_content(&self, field: &'static str, content: &str) -> Result<(), MemoryError> {
        if content.trim().is_empty() {
            return Err(MemoryError::InvalidConfig {
                field,
                reason: "content must not be empty".to_string(),
            });
        }

        let limit = self.inner.config.limits.max_content_bytes;
        if content.len() > limit {
            return Err(MemoryError::ContentTooLarge {
                size: content.len(),
                limit,
            });
        }

        Ok(())
    }

    fn validate_confidence(confidence: f32) -> Result<(), MemoryError> {
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            return Err(MemoryError::InvalidConfig {
                field: "episodes.confidence",
                reason: "confidence must be finite and within [0.0, 1.0]".to_string(),
            });
        }
        Ok(())
    }

    // ─── HNSW Management ───────────────────────────────────────

    /// Rebuild feature-gated TurboQuant artifacts from authoritative SQLite f32 embeddings.
    #[cfg(feature = "turbo-quant-codec")]
    pub async fn rebuild_vector_artifacts(
        &self,
    ) -> Result<VectorArtifactBuildReceiptV1, MemoryError> {
        let dim = self.inner.config.embedding.dimensions;
        let search = self.inner.config.search.clone();
        self.with_write_conn(move |conn| {
            db::rebuild_turbo_quant_artifacts(
                conn,
                dim,
                search.turbo_quant_bits,
                search.turbo_quant_projections,
                search.turbo_quant_seed,
            )
        })
        .await
    }

    /// Rebuild the HNSW index from SQLite f32 embeddings.
    ///
    /// Call this if sidecar files are missing, corrupted, or after `reembed_all()`.
    #[cfg(feature = "hnsw")]
    pub async fn rebuild_hnsw_index(&self) -> Result<(), MemoryError> {
        tracing::info!("Rebuilding HNSW index from SQLite embeddings...");
        let hnsw_config = self.inner.config.hnsw.clone();
        let new_index = self
            .with_read_conn(move |conn| hnsw_ops::rebuild_hnsw_from_sqlite(conn, &hnsw_config))
            .await?;

        {
            let mut guard = self
                .inner
                .hnsw_index
                .write()
                .unwrap_or_else(|e| e.into_inner());
            *guard = new_index.clone();
        }

        hnsw_ops::save_hnsw_sidecar(
            &new_index,
            &self.inner.paths.hnsw_dir,
            &self.inner.paths.hnsw_basename,
        )?;
        self.inner.pool.with_write_conn(|conn| {
            new_index.flush_keymap(conn)?;
            db::clear_all_pending_index_ops(conn)?;
            db::set_sidecar_dirty(conn, false)?;
            Ok(())
        })?;

        tracing::info!(active = new_index.len(), "HNSW index rebuilt");

        Ok(())
    }

    /// Opportunistically flush HNSW if the configured interval has elapsed.
    ///
    /// Cheap no-op when `flush_interval_secs` is None or the interval hasn't
    /// elapsed yet (just an atomic load + epoch comparison).
    #[cfg(feature = "hnsw")]
    fn maybe_flush_hnsw(&self) {
        if let Some(interval) = self.inner.config.hnsw.flush_interval_secs {
            let guard = self
                .inner
                .hnsw_index
                .read()
                .unwrap_or_else(|e| e.into_inner());
            if guard.should_flush(interval) {
                drop(guard); // release read lock before flushing
                if let Err(e) = self.flush_hnsw() {
                    tracing::warn!("Opportunistic HNSW flush failed: {}", e);
                } else {
                    let guard = self
                        .inner
                        .hnsw_index
                        .read()
                        .unwrap_or_else(|e| e.into_inner());
                    guard.update_last_flush_epoch();
                    tracing::info!("Opportunistic HNSW flush completed");
                }
            }
        }
    }

    /// Persist the HNSW graph, vector data, and key mappings to disk.
    ///
    /// Called automatically on drop, but can be called explicitly for durability.
    #[cfg(feature = "hnsw")]
    pub fn flush_hnsw(&self) -> Result<(), MemoryError> {
        let pending_ops = self.inner.pool.with_read_conn(db::pending_index_op_count)?;
        if pending_ops > 0 {
            tracing::info!(
                pending_ops,
                "Flushing HNSW via authoritative SQLite rebuild because pending durable sidecar work exists"
            );
            let rebuilt = hnsw_ops::recover_hnsw_sidecar_sync(
                &self.inner.pool,
                &self.inner.paths,
                &self.inner.config.hnsw,
            )?;
            let mut guard = self
                .inner
                .hnsw_index
                .write()
                .unwrap_or_else(|e| e.into_inner());
            *guard = rebuilt;
            return Ok(());
        }

        let index = self
            .inner
            .hnsw_index
            .write()
            .unwrap_or_else(|e| e.into_inner());
        hnsw_ops::save_hnsw_sidecar(
            &index,
            &self.inner.paths.hnsw_dir,
            &self.inner.paths.hnsw_basename,
        )?;

        // Flush key mappings to SQLite
        self.inner.pool.with_write_conn(|conn| {
            index.flush_keymap(conn)?;
            db::clear_all_pending_index_ops(conn)?;
            db::set_sidecar_dirty(conn, false)?;
            Ok(())
        })?;
        Ok(())
    }

    /// Compact the HNSW index by rebuilding without tombstones.
    ///
    /// Only rebuilds if the deleted ratio exceeds the compaction threshold.
    #[cfg(feature = "hnsw")]
    pub async fn compact_hnsw(&self) -> Result<(), MemoryError> {
        if !self
            .inner
            .hnsw_index
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .needs_compaction()
        {
            tracing::info!("HNSW compaction not needed (deleted ratio below threshold)");
            return Ok(());
        }
        self.rebuild_hnsw_index().await
    }

    // ─── Integrity & Diagnostics ────────────────────────────────

    /// Verify database integrity.
    ///
    /// In `Quick` mode, checks table existence and row counts.
    /// In `Full` mode, also verifies FTS consistency and runs SQLite integrity_check.
    pub async fn verify_integrity(
        &self,
        mode: db::VerifyMode,
    ) -> Result<db::IntegrityReport, MemoryError> {
        let use_writer = mode == db::VerifyMode::Full;
        let mut report = if use_writer {
            self.with_write_conn(move |conn| db::verify_integrity_sync(conn, mode))
                .await?
        } else {
            self.with_read_conn(move |conn| db::verify_integrity_sync(conn, mode))
                .await?
        };

        #[cfg(feature = "hnsw")]
        {
            let hnsw_vectors = self
                .inner
                .hnsw_index
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .vector_snapshot();
            let hnsw_dims = self.inner.config.embedding.dimensions;
            let hnsw_files_exist = self.inner.paths.hnsw_files_exist();

            let hnsw_issues = if use_writer {
                let hnsw_vectors = hnsw_vectors.clone();
                self.with_write_conn(move |conn| {
                    verify_hnsw_key_level_integrity(
                        conn,
                        hnsw_dims,
                        &hnsw_vectors,
                        hnsw_files_exist,
                    )
                })
                .await?
            } else {
                let hnsw_vectors = hnsw_vectors.clone();
                self.with_read_conn(move |conn| {
                    verify_hnsw_key_level_integrity(
                        conn,
                        hnsw_dims,
                        &hnsw_vectors,
                        hnsw_files_exist,
                    )
                })
                .await?
            };
            report.issues.extend(hnsw_issues);
        }

        report.ok = report.issues.is_empty();
        Ok(report)
    }

    /// Reconcile detected integrity issues.
    ///
    /// - `ReportOnly`: no-op, just returns the integrity report.
    /// - `RebuildFts`: rebuilds all FTS indexes from source data.
    /// - `ReEmbed`: not yet implemented (requires async embedding calls).
    pub async fn reconcile(
        &self,
        action: db::ReconcileAction,
    ) -> Result<db::IntegrityReport, MemoryError> {
        match action {
            db::ReconcileAction::ReportOnly => self.verify_integrity(db::VerifyMode::Full).await,
            db::ReconcileAction::RebuildFts => {
                self.with_write_conn(db::reconcile_fts).await?;
                #[cfg(feature = "hnsw")]
                self.sync_pending_hnsw_ops_best_effort("reconcile_rebuild_fts")
                    .await;
                self.verify_integrity(db::VerifyMode::Full).await
            }
            db::ReconcileAction::ReEmbed => {
                self.reembed_all().await?;
                self.verify_integrity(db::VerifyMode::Full).await
            }
        }
    }

    /// Get the current configuration.
    pub fn config(&self) -> &MemoryConfig {
        &self.inner.config
    }

    /// View the store as a derived graph over documents, chunks, facts, sessions, messages,
    /// episodes, namespaces, and semantic similarity edges.
    pub fn graph_view(&self) -> Arc<dyn GraphView> {
        graph::graph_view(self.inner.clone())
    }

    // ─── Search ─────────────────────────────────────────────────

    /// Hybrid search across facts, document chunks, and searchable episodes.
    pub async fn search(
        &self,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
    ) -> Result<Vec<SearchResult>, MemoryError> {
        Ok(self
            .search_with_context(
                query,
                top_k,
                namespaces,
                source_types,
                SearchContext::default_now(),
            )
            .await?
            .results)
    }

    /// Hybrid search with an explicit deterministic context and optional receipt.
    pub async fn search_with_context(
        &self,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
        context: SearchContext,
    ) -> Result<SearchResponse, MemoryError> {
        let k = top_k
            .unwrap_or(self.inner.config.search.default_top_k)
            .min(MAX_TOP_K);

        let query_embedding = self.embed_text_internal(query).await?;

        #[cfg(feature = "hnsw")]
        let hnsw_hits = if context.exactness_profile == ExactnessProfile::PreferExact
            || self.inner.config.search.uses_turbo_quant_backend()
        {
            Vec::new()
        } else {
            let candidates = self
                .inner
                .config
                .search
                .candidate_pool_size
                .max(k.saturating_mul(3))
                .min(MAX_HNSW_CANDIDATES);
            self.hnsw_search_blocking(query_embedding.clone(), candidates)
                .await
        };

        let q = query.to_string();
        let config = self.inner.config.search.clone();
        let ns_owned = to_owned_string_vec(namespaces);
        let st_owned: Option<Vec<SearchSourceType>> = source_types.map(|s| s.to_vec());
        let context_owned = context.clone();

        #[cfg(feature = "hnsw")]
        let hnsw_hits_owned = hnsw_hits;

        let response = self
            .with_read_conn(move |conn| {
                if db::is_embeddings_dirty(conn)? {
                    tracing::warn!(
                        "Embeddings are stale after model change — search quality is degraded. \
                     Call reembed_all() to regenerate embeddings."
                    );
                }
                let ns_refs = as_str_slice(&ns_owned);
                let ns_slice: Option<&[&str]> = ns_refs.as_deref();
                let st_slice: Option<&[SearchSourceType]> = st_owned.as_deref();

                #[cfg(feature = "hnsw")]
                {
                    let mut execution = if hnsw_hits_owned.is_empty() {
                        search::hybrid_search_detailed_with_context(
                            conn,
                            &q,
                            &query_embedding,
                            &config,
                            &context_owned,
                            k,
                            ns_slice,
                            st_slice,
                            None,
                        )
                    } else {
                        search::hybrid_search_with_hnsw_detailed_with_context(
                            conn,
                            &q,
                            &query_embedding,
                            &config,
                            &context_owned,
                            k,
                            ns_slice,
                            st_slice,
                            None,
                            &hnsw_hits_owned,
                        )
                    }?;
                    if context_owned.receipts_enabled()
                        && context_owned.exactness_profile == ExactnessProfile::PreferExact
                    {
                        if let Some(receipt) = execution.receipt.as_mut() {
                            receipt.search_profile = "hybrid_prefer_exact".to_string();
                        }
                    }
                    Ok(SearchResponse {
                        results: execution
                            .results
                            .into_iter()
                            .map(|result| result.result)
                            .collect(),
                        receipt: execution.receipt,
                    })
                }
                #[cfg(not(feature = "hnsw"))]
                {
                    let execution = search::hybrid_search_detailed_with_context(
                        conn,
                        &q,
                        &query_embedding,
                        &config,
                        &context_owned,
                        k,
                        ns_slice,
                        st_slice,
                        None,
                    )?;
                    Ok(SearchResponse {
                        results: execution
                            .results
                            .into_iter()
                            .map(|result| result.result)
                            .collect(),
                        receipt: execution.receipt,
                    })
                }
            })
            .await?;
        if let Some(receipt) = &response.receipt {
            self.persist_search_receipt(receipt).await?;
        }
        Ok(response)
    }

    /// Full-text search only (no embeddings needed).
    pub async fn search_fts_only(
        &self,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
    ) -> Result<Vec<SearchResult>, MemoryError> {
        let k = top_k
            .unwrap_or(self.inner.config.search.default_top_k)
            .min(MAX_TOP_K);
        let q = query.to_string();
        let config = self.inner.config.search.clone();
        let ns_owned = to_owned_string_vec(namespaces);
        let st_owned: Option<Vec<SearchSourceType>> = source_types.map(|s| s.to_vec());
        self.with_read_conn(move |conn| {
            let ns_refs = as_str_slice(&ns_owned);
            let ns_slice: Option<&[&str]> = ns_refs.as_deref();
            let st_slice: Option<&[SearchSourceType]> = st_owned.as_deref();
            search::fts_only_search(conn, &q, &config, k, ns_slice, st_slice, None)
        })
        .await
    }

    /// Vector similarity search only (no FTS).
    pub async fn search_vector_only(
        &self,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
    ) -> Result<Vec<SearchResult>, MemoryError> {
        Ok(self
            .search_vector_only_with_context(
                query,
                top_k,
                namespaces,
                source_types,
                SearchContext::default_now(),
            )
            .await?
            .results)
    }

    /// Vector similarity search with an explicit deterministic context and optional receipt.
    pub async fn search_vector_only_with_context(
        &self,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
        context: SearchContext,
    ) -> Result<SearchResponse, MemoryError> {
        let k = top_k
            .unwrap_or(self.inner.config.search.default_top_k)
            .min(MAX_TOP_K);
        let query_embedding = self.embed_text_internal(query).await?;

        #[cfg(feature = "hnsw")]
        let hnsw_hits = if context.exactness_profile == ExactnessProfile::PreferExact
            || self.inner.config.search.uses_turbo_quant_backend()
        {
            Vec::new()
        } else {
            let candidates = self
                .inner
                .config
                .search
                .candidate_pool_size
                .max(k.saturating_mul(3))
                .min(MAX_HNSW_CANDIDATES);
            self.hnsw_search_blocking(query_embedding.clone(), candidates)
                .await
        };

        let config = self.inner.config.search.clone();
        let ns_owned = to_owned_string_vec(namespaces);
        let st_owned: Option<Vec<SearchSourceType>> = source_types.map(|s| s.to_vec());
        let context_owned = context.clone();

        #[cfg(feature = "hnsw")]
        let hnsw_hits_owned = hnsw_hits;

        let response = self
            .with_read_conn(move |conn| {
                if db::is_embeddings_dirty(conn)? {
                    tracing::warn!(
                        "Embeddings are stale after model change — search quality is degraded. \
                     Call reembed_all() to regenerate embeddings."
                    );
                }
                let ns_refs = as_str_slice(&ns_owned);
                let ns_slice: Option<&[&str]> = ns_refs.as_deref();
                let st_slice: Option<&[SearchSourceType]> = st_owned.as_deref();

                #[cfg(feature = "hnsw")]
                {
                    let mut execution = if hnsw_hits_owned.is_empty() {
                        search::vector_only_search_detailed_with_context(
                            conn,
                            &query_embedding,
                            &config,
                            &context_owned,
                            k,
                            ns_slice,
                            st_slice,
                            None,
                        )
                    } else {
                        search::vector_only_search_with_hnsw_detailed_with_context(
                            conn,
                            &query_embedding,
                            &config,
                            &context_owned,
                            k,
                            ns_slice,
                            st_slice,
                            None,
                            &hnsw_hits_owned,
                        )
                    }?;
                    if context_owned.receipts_enabled()
                        && context_owned.exactness_profile == ExactnessProfile::PreferExact
                    {
                        if let Some(receipt) = execution.receipt.as_mut() {
                            receipt.search_profile = "vector_only_prefer_exact".to_string();
                        }
                    }
                    Ok(SearchResponse {
                        results: execution
                            .results
                            .into_iter()
                            .map(|result| result.result)
                            .collect(),
                        receipt: execution.receipt,
                    })
                }
                #[cfg(not(feature = "hnsw"))]
                {
                    let execution = search::vector_only_search_detailed_with_context(
                        conn,
                        &query_embedding,
                        &config,
                        &context_owned,
                        k,
                        ns_slice,
                        st_slice,
                        None,
                    )?;
                    Ok(SearchResponse {
                        results: execution
                            .results
                            .into_iter()
                            .map(|result| result.result)
                            .collect(),
                        receipt: execution.receipt,
                    })
                }
            })
            .await?;
        if let Some(receipt) = &response.receipt {
            self.persist_search_receipt(receipt).await?;
        }
        Ok(response)
    }

    // ─── Explainable Search ───────────────────────────────────

    /// Search with full score breakdown for each result.
    pub async fn search_explained(
        &self,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
    ) -> Result<Vec<types::ExplainedResult>, MemoryError> {
        Ok(self
            .search_explained_with_context(
                query,
                top_k,
                namespaces,
                source_types,
                SearchContext::default_now(),
            )
            .await?
            .results)
    }

    /// Search with full score breakdown under an explicit deterministic context.
    pub async fn search_explained_with_context(
        &self,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
        context: SearchContext,
    ) -> Result<types::ExplainedSearchResponse, MemoryError> {
        let k = top_k
            .unwrap_or(self.inner.config.search.default_top_k)
            .min(MAX_TOP_K);
        let query_embedding = self.embed_text_internal(query).await?;

        #[cfg(feature = "hnsw")]
        let hnsw_hits = if context.exactness_profile == ExactnessProfile::PreferExact {
            Vec::new()
        } else {
            let candidates = self
                .inner
                .config
                .search
                .candidate_pool_size
                .max(k.saturating_mul(3))
                .min(MAX_HNSW_CANDIDATES);
            self.hnsw_search_blocking(query_embedding.clone(), candidates)
                .await
        };

        let q = query.to_string();
        let config = self.inner.config.search.clone();
        let ns_owned = to_owned_string_vec(namespaces);
        let st_owned: Option<Vec<SearchSourceType>> = source_types.map(|value| value.to_vec());
        let context_owned = context.clone();

        #[cfg(feature = "hnsw")]
        let hnsw_hits_owned = hnsw_hits;

        let response = self
            .with_read_conn(move |conn| {
                let ns_refs = as_str_slice(&ns_owned);
                let ns_slice: Option<&[&str]> = ns_refs.as_deref();
                let st_slice: Option<&[SearchSourceType]> = st_owned.as_deref();

                #[cfg(feature = "hnsw")]
                {
                    let mut execution = if hnsw_hits_owned.is_empty() {
                        search::hybrid_search_detailed_with_context(
                            conn,
                            &q,
                            &query_embedding,
                            &config,
                            &context_owned,
                            k,
                            ns_slice,
                            st_slice,
                            None,
                        )
                    } else {
                        search::hybrid_search_with_hnsw_detailed_with_context(
                            conn,
                            &q,
                            &query_embedding,
                            &config,
                            &context_owned,
                            k,
                            ns_slice,
                            st_slice,
                            None,
                            &hnsw_hits_owned,
                        )
                    }?;
                    if context_owned.receipts_enabled()
                        && context_owned.exactness_profile == ExactnessProfile::PreferExact
                    {
                        if let Some(receipt) = execution.receipt.as_mut() {
                            receipt.search_profile = "hybrid_prefer_exact".to_string();
                        }
                    }
                    Ok(types::ExplainedSearchResponse {
                        results: execution.results,
                        receipt: execution.receipt,
                    })
                }
                #[cfg(not(feature = "hnsw"))]
                {
                    let execution = search::hybrid_search_detailed_with_context(
                        conn,
                        &q,
                        &query_embedding,
                        &config,
                        &context_owned,
                        k,
                        ns_slice,
                        st_slice,
                        None,
                    )?;
                    Ok(types::ExplainedSearchResponse {
                        results: execution.results,
                        receipt: execution.receipt,
                    })
                }
            })
            .await?;
        if let Some(receipt) = &response.receipt {
            self.persist_search_receipt(receipt).await?;
        }
        Ok(response)
    }

    /// Load a durable search receipt by receipt/request ID.
    pub async fn get_search_receipt(
        &self,
        receipt_id: &str,
    ) -> Result<Option<VectorSearchReceiptV1>, MemoryError> {
        let receipt_id = receipt_id.to_string();
        self.with_read_conn(move |conn| db::get_search_receipt(conn, &receipt_id))
            .await
    }

    /// Replay a durable search receipt with caller-supplied query text and filters.
    ///
    /// Receipts intentionally do not store query text or filter values. The
    /// caller supplies those inputs, and the stored receipt supplies the
    /// deterministic evaluation time and retrieval family for comparison.
    pub async fn replay_search_receipt(
        &self,
        receipt_id: &str,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
    ) -> Result<SearchReplayReportV1, MemoryError> {
        let original_receipt = self.get_search_receipt(receipt_id).await?.ok_or_else(|| {
            MemoryError::SearchReceiptNotFound {
                receipt_id: receipt_id.to_string(),
            }
        })?;

        let vector_only = original_receipt.search_profile.starts_with("vector_only");
        let replay_top_k = top_k.or_else(|| Some(original_receipt.result_ids.len().max(1)));
        let replay_receipt_id = format!("{receipt_id}:replay:{}", uuid::Uuid::new_v4());
        let mut context = SearchContext::at(original_receipt.evaluation_time);
        context.receipt_mode = ReceiptMode::ReturnReceipt;
        context.request_id = Some(replay_receipt_id.clone());
        context.trace_id = original_receipt.trace_id.clone();
        context.attempt_family_id = original_receipt
            .attempt_family_id
            .clone()
            .or_else(|| Some(original_receipt.receipt_id.clone()));
        context.attempt_id = Some(replay_receipt_id.clone());
        context.replay_of = Some(original_receipt.receipt_id.clone());
        context.query_text_digest = original_receipt.query_text_digest.clone();
        context.query_input_digest = original_receipt.query_input_digest.clone();
        context.filter_digest = original_receipt.filter_digest.clone();
        context.redaction_state = original_receipt.redaction_state.clone();
        context.budget_id = original_receipt.budget_id.clone();
        context.exactness_profile = if original_receipt.approximate {
            ExactnessProfile::AllowApproximate
        } else {
            ExactnessProfile::PreferExact
        };

        let replay_response = if vector_only {
            self.search_vector_only_with_context(
                query,
                replay_top_k,
                namespaces,
                source_types,
                context,
            )
            .await?
        } else {
            self.search_with_context(query, replay_top_k, namespaces, source_types, context)
                .await?
        };
        let replay_receipt = replay_response
            .receipt
            .ok_or_else(|| MemoryError::Other("replay did not produce a receipt".to_string()))?;

        let query_embedding_digest_matches =
            original_receipt.query_embedding_digest == replay_receipt.query_embedding_digest;
        let result_ids_match = original_receipt.result_ids == replay_receipt.result_ids;
        let missing_result_ids = original_receipt
            .result_ids
            .iter()
            .filter(|id| !replay_receipt.result_ids.contains(*id))
            .cloned()
            .collect();
        let added_result_ids = replay_receipt
            .result_ids
            .iter()
            .filter(|id| !original_receipt.result_ids.contains(*id))
            .cloned()
            .collect();

        Ok(SearchReplayReportV1 {
            receipt_id: original_receipt.receipt_id.clone(),
            replay_receipt_id,
            original_receipt,
            replay_receipt,
            query_embedding_digest_matches,
            result_ids_match,
            missing_result_ids,
            added_result_ids,
            vector_only,
        })
    }

    // ─── Embedding Displacement ───────────────────────────────

    /// Compute embedding displacement between two texts.
    pub async fn embedding_displacement(
        &self,
        text_a: &str,
        text_b: &str,
    ) -> Result<types::EmbeddingDisplacement, MemoryError> {
        let emb_a = self.embed_text_internal(text_a).await?;
        let emb_b = self.embed_text_internal(text_b).await?;
        Self::embedding_displacement_from_vecs(&emb_a, &emb_b)
    }

    /// Compute embedding displacement from pre-computed vectors.
    pub fn embedding_displacement_from_vecs(
        a: &[f32],
        b: &[f32],
    ) -> Result<types::EmbeddingDisplacement, MemoryError> {
        if a.len() != b.len() {
            return Err(MemoryError::DimensionMismatch {
                expected: a.len(),
                actual: b.len(),
            });
        }
        let cosine_sim = search::cosine_similarity(a, b)?;

        let euclidean_dist: f32 = a
            .iter()
            .zip(b.iter())
            .map(|(x, y)| (x - y) * (x - y))
            .sum::<f32>()
            .sqrt();

        let mag_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let mag_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        Ok(types::EmbeddingDisplacement {
            cosine_similarity: cosine_sim,
            euclidean_distance: euclidean_dist,
            magnitude_a: mag_a,
            magnitude_b: mag_b,
        })
    }

    // ─── Utility ────────────────────────────────────────────────

    /// Chunk text using the configured strategy and token counter.
    pub fn chunk_text(&self, text: &str) -> Vec<TextChunk> {
        chunker::chunk_text(
            text,
            &self.inner.config.chunking,
            self.inner.token_counter.as_ref(),
        )
    }

    /// Embed a single text via the configured provider.
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
        self.embed_text_internal(text).await
    }

    /// Embed multiple texts in a batch.
    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, MemoryError> {
        let owned: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        self.embed_batch_internal(owned).await
    }

    /// Get database statistics.
    pub async fn stats(&self) -> Result<MemoryStats, MemoryError> {
        let db_path = self.inner.paths.sqlite_path.clone();
        self.with_read_conn(move |conn| {
            let total_facts: u64 =
                conn.query_row("SELECT COUNT(*) FROM facts", [], |r| r.get(0))?;
            let total_documents: u64 =
                conn.query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))?;
            let total_chunks: u64 =
                conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
            let total_sessions: u64 =
                conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))?;
            let total_messages: u64 =
                conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;

            let db_size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);

            let (model, dims): (Option<String>, Option<usize>) = conn
                .query_row(
                    "SELECT model_name, dimensions FROM embedding_metadata WHERE id = 1",
                    [],
                    |r| Ok((Some(r.get(0)?), Some(r.get(1)?))),
                )
                .unwrap_or((None, None));

            Ok(MemoryStats {
                total_facts,
                total_documents,
                total_chunks,
                total_sessions,
                total_messages,
                database_size_bytes: db_size,
                embedding_model: model,
                embedding_dimensions: dims,
            })
        })
        .await
    }

    /// Return distinct scope_domain values stored in document metadata.
    ///
    /// Queries `json_extract(metadata, '$.scope_domain')` across all documents
    /// and returns the unique non-null values. Used by the Recall app to populate
    /// the scope picker dynamically instead of relying on a hardcoded list.
    pub async fn list_scope_domains(&self) -> Result<Vec<String>, MemoryError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT json_extract(metadata, '$.scope_domain') \
                 FROM documents \
                 WHERE json_extract(metadata, '$.scope_domain') IS NOT NULL",
            )?;
            let domains: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .collect();
            Ok(domains)
        })
        .await
    }

    /// Check if embeddings need re-generation after a model change.
    pub async fn embeddings_are_dirty(&self) -> Result<bool, MemoryError> {
        self.with_read_conn(db::is_embeddings_dirty).await
    }

    /// Re-embed all facts, chunks, messages, and episodes. Call after changing embedding models.
    pub async fn reembed_all(&self) -> Result<usize, MemoryError> {
        let mut count = 0usize;
        let batch_size = self.inner.config.embedding.batch_size;
        let dims = self.inner.config.embedding.dimensions;

        // ─── Facts ──────────────────────────────────────────────────
        let fact_contents: Vec<(String, String)> = self
            .with_read_conn(|conn| {
                let mut stmt = conn.prepare("SELECT id, content FROM facts")?;
                let result = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(result)
            })
            .await?;

        let mut fact_count = 0usize;
        for batch in fact_contents.chunks(batch_size) {
            let texts: Vec<String> = batch.iter().map(|(_, c)| c.clone()).collect();
            let embeddings = self.embed_batch_internal(texts).await?;
            for embedding in &embeddings {
                self.validate_embedding_dimensions(embedding)?;
            }

            let quantizer = Quantizer::new(dims);
            let updates: Vec<(String, Vec<u8>, Option<Vec<u8>>)> = batch
                .iter()
                .zip(embeddings.iter())
                .map(|((id, _), emb)| {
                    // INTENTIONAL: q8 quantization is an optional search optimization; missing q8 is non-fatal
                    let q8 = quantizer
                        .quantize(emb)
                        .map(|qv| quantize::pack_quantized(&qv))
                        .ok();
                    (id.clone(), db::embedding_to_bytes(emb), q8)
                })
                .collect();

            self.with_write_conn(move |conn| {
                db::with_transaction(conn, |tx| {
                    for (fid, bytes, q8) in &updates {
                        tx.execute(
                            "UPDATE facts SET embedding = ?1, embedding_q8 = ?2, updated_at = datetime('now') WHERE id = ?3",
                            rusqlite::params![bytes, q8.as_deref(), fid],
                        )?;
                        #[cfg(feature = "hnsw")]
                        db::queue_pending_index_op(
                            tx,
                            &format!("fact:{fid}"),
                            "fact",
                            db::IndexOpKind::Upsert,
                        )?;
                        db::invalidate_derived_vector_artifact(tx, &format!("fact:{fid}"))?;
                    }
                    Ok(())
                })
            })
            .await?;

            fact_count += batch.len();
            count += batch.len();
            if fact_count % 100 == 0 || fact_count == count {
                tracing::info!(fact_count, "Re-embedded {} facts so far", fact_count);
            }
        }

        // ─── Chunks ─────────────────────────────────────────────────
        let chunk_data: Vec<(String, String)> = self
            .with_read_conn(|conn| {
                let mut stmt = conn.prepare("SELECT id, content FROM chunks")?;
                let result = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(result)
            })
            .await?;

        let mut chunk_count = 0usize;
        for batch in chunk_data.chunks(batch_size) {
            let texts: Vec<String> = batch.iter().map(|(_, c)| c.clone()).collect();
            let embeddings = self.embed_batch_internal(texts).await?;
            for embedding in &embeddings {
                self.validate_embedding_dimensions(embedding)?;
            }

            let quantizer = Quantizer::new(dims);
            let updates: Vec<(String, Vec<u8>, Option<Vec<u8>>)> = batch
                .iter()
                .zip(embeddings.iter())
                .map(|((id, _), emb)| {
                    // INTENTIONAL: q8 quantization is an optional search optimization; missing q8 is non-fatal
                    let q8 = quantizer
                        .quantize(emb)
                        .map(|qv| quantize::pack_quantized(&qv))
                        .ok();
                    (id.clone(), db::embedding_to_bytes(emb), q8)
                })
                .collect();

            self.with_write_conn(move |conn| {
                db::with_transaction(conn, |tx| {
                    for (cid, bytes, q8) in &updates {
                        tx.execute(
                            "UPDATE chunks SET embedding = ?1, embedding_q8 = ?2 WHERE id = ?3",
                            rusqlite::params![bytes, q8.as_deref(), cid],
                        )?;
                        #[cfg(feature = "hnsw")]
                        db::queue_pending_index_op(
                            tx,
                            &format!("chunk:{cid}"),
                            "chunk",
                            db::IndexOpKind::Upsert,
                        )?;
                        db::invalidate_derived_vector_artifact(tx, &format!("chunk:{cid}"))?;
                    }
                    Ok(())
                })
            })
            .await?;

            chunk_count += batch.len();
            count += batch.len();
            if chunk_count % 100 == 0 {
                tracing::info!(chunk_count, "Re-embedded {} chunks so far", chunk_count);
            }
        }

        // ─── Messages ───────────────────────────────────────────────
        let message_data: Vec<(i64, String)> = self
            .with_read_conn(|conn| {
                let mut stmt = conn.prepare("SELECT id, content FROM messages")?;
                let result = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(result)
            })
            .await?;

        let mut msg_count = 0usize;
        for batch in message_data.chunks(batch_size) {
            let texts: Vec<String> = batch.iter().map(|(_, c)| c.clone()).collect();
            let embeddings = self.embed_batch_internal(texts).await?;
            for embedding in &embeddings {
                self.validate_embedding_dimensions(embedding)?;
            }

            let quantizer = Quantizer::new(dims);
            let updates: Vec<(i64, Vec<u8>, Option<Vec<u8>>)> = batch
                .iter()
                .zip(embeddings.iter())
                .map(|((id, _), emb)| {
                    // INTENTIONAL: q8 quantization is an optional search optimization; missing q8 is non-fatal
                    let q8 = quantizer
                        .quantize(emb)
                        .map(|qv| quantize::pack_quantized(&qv))
                        .ok();
                    (*id, db::embedding_to_bytes(emb), q8)
                })
                .collect();

            self.with_write_conn(move |conn| {
                db::with_transaction(conn, |tx| {
                    for (mid, bytes, q8) in &updates {
                        tx.execute(
                            "UPDATE messages SET embedding = ?1, embedding_q8 = ?2 WHERE id = ?3",
                            rusqlite::params![bytes, q8.as_deref(), mid],
                        )?;
                        #[cfg(feature = "hnsw")]
                        db::queue_pending_index_op(
                            tx,
                            &format!("msg:{mid}"),
                            "message",
                            db::IndexOpKind::Upsert,
                        )?;
                        db::invalidate_derived_vector_artifact(tx, &format!("msg:{mid}"))?;
                    }
                    Ok(())
                })
            })
            .await?;

            msg_count += batch.len();
            count += batch.len();
            if msg_count % 100 == 0 {
                tracing::info!(msg_count, "Re-embedded {} messages so far", msg_count);
            }
        }

        // ─── Episodes ───────────────────────────────────────────────
        let episode_data: Vec<(String, String)> = self
            .with_read_conn(|conn| {
                let mut stmt = conn.prepare("SELECT episode_id, search_text FROM episodes")?;
                let result = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(result)
            })
            .await?;

        let mut episode_count = 0usize;
        for batch in episode_data.chunks(batch_size) {
            let texts: Vec<String> = batch.iter().map(|(_, text)| text.clone()).collect();
            let embeddings = self.embed_batch_internal(texts).await?;
            for embedding in &embeddings {
                self.validate_embedding_dimensions(embedding)?;
            }

            let quantizer = Quantizer::new(dims);
            let updates: Vec<(String, Vec<u8>, Option<Vec<u8>>)> = batch
                .iter()
                .zip(embeddings.iter())
                .map(|((episode_id, _), embedding)| {
                    // INTENTIONAL: q8 quantization is an optional search optimization; missing q8 is non-fatal
                    let q8 = quantizer
                        .quantize(embedding)
                        .map(|vector| quantize::pack_quantized(&vector))
                        .ok();
                    (episode_id.clone(), db::embedding_to_bytes(embedding), q8)
                })
                .collect();

            self.with_write_conn(move |conn| {
                db::with_transaction(conn, |tx| {
                    for (episode_id, bytes, q8) in &updates {
                        tx.execute(
                            "UPDATE episodes
                             SET embedding = ?1,
                                 embedding_q8 = ?2,
                                 updated_at = datetime('now')
                             WHERE episode_id = ?3",
                            rusqlite::params![bytes, q8.as_deref(), episode_id],
                        )?;
                        #[cfg(feature = "hnsw")]
                        db::queue_pending_index_op(
                            tx,
                            &episodes::episode_item_key(episode_id),
                            "episode",
                            db::IndexOpKind::Upsert,
                        )?;
                        db::invalidate_derived_vector_artifact(
                            tx,
                            &episodes::episode_item_key(episode_id),
                        )?;
                    }
                    Ok(())
                })
            })
            .await?;

            episode_count += batch.len();
            count += batch.len();
            if episode_count % 100 == 0 {
                tracing::info!(
                    episode_count,
                    "Re-embedded {} episodes so far",
                    episode_count
                );
            }
        }

        // Clear the dirty flag
        self.with_write_conn(db::clear_embeddings_dirty).await?;

        tracing::info!(
            facts = fact_count,
            chunks = chunk_count,
            messages = msg_count,
            episodes = episode_count,
            total = count,
            "Re-embedding complete"
        );

        // Rebuild HNSW after re-embedding
        #[cfg(feature = "hnsw")]
        {
            tracing::info!("Rebuilding HNSW index after re-embedding...");
            self.rebuild_hnsw_index().await?;
        }

        Ok(count)
    }

    /// Vacuum the database (reclaim space after deletions).
    pub async fn vacuum(&self) -> Result<(), MemoryError> {
        self.with_write_conn(|conn| {
            conn.execute_batch("VACUUM")?;
            Ok(())
        })
        .await
    }

    // ─── Projection Import ─────────────────────────────────────

    /// Import a projection envelope atomically (V10 legacy path).
    ///
    /// ## Phase status: compatibility / migration-only
    ///
    /// This method is the V10 legacy import path. New integrations should use
    /// [`import_projection_batch()`](Self::import_projection_batch) instead,
    /// which accepts the canonical `ProjectionImportBatchV3` format from
    /// `forge-memory-bridge`.
    ///
    /// **Removal condition**: removed when all callers migrate to the bridge pipeline.
    ///
    /// **Idempotent**: re-importing the same envelope (same `envelope_id` +
    /// `schema_version` + `content_digest`) returns a receipt with
    /// `was_duplicate = true` and does not modify data.
    ///
    /// **Atomic**: all records are committed in a single transaction. On any
    /// failure the entire import is rolled back — no partial visibility.
    ///
    /// **Provenance**: each imported record's metadata is tagged with the
    /// envelope_id and source_authority for traceability.
    #[deprecated(
        since = "0.5.0",
        note = "Legacy V10 import envelope path is compatibility-only. Use `import_projection_batch()` and `ProjectionImportBatchV3` on the canonical lane."
    )]
    #[doc(hidden)]
    #[allow(deprecated)]
    pub async fn import_envelope(
        &self,
        envelope: &projection_import::ImportEnvelope,
    ) -> Result<projection_import::ImportReceipt, MemoryError> {
        projection_legacy_compat::import_envelope(self, envelope).await
    }

    /// Check whether an envelope has already been imported.
    #[deprecated(
        since = "0.5.0",
        note = "Legacy V10 import envelope status reads are compatibility-only. Prefer the projection import log."
    )]
    #[doc(hidden)]
    #[allow(deprecated)]
    pub async fn import_status(
        &self,
        envelope_id: &projection_import::EnvelopeId,
    ) -> Result<Vec<projection_import::ImportReceipt>, MemoryError> {
        projection_legacy_compat::import_status(self, envelope_id).await
    }

    /// List recent imports, optionally filtered by namespace.
    #[deprecated(
        since = "0.5.0",
        note = "Legacy V10 import log access is compatibility-only. Prefer new projection-import metadata."
    )]
    #[doc(hidden)]
    #[allow(deprecated)]
    pub async fn list_imports(
        &self,
        namespace: Option<&str>,
        limit: usize,
    ) -> Result<Vec<projection_import::ImportReceipt>, MemoryError> {
        projection_legacy_compat::list_imports(self, namespace, limit).await
    }

    /// Get the most recent successful import timestamp for a namespace.
    #[allow(deprecated)]
    pub async fn last_import_at(&self, namespace: &str) -> Result<Option<String>, MemoryError> {
        projection_legacy_compat::last_import_at(self, namespace).await
    }

    /// Query imported claim projection rows through the supported public read surface.
    pub async fn query_claim_versions(
        &self,
        query: ProjectionQuery,
    ) -> Result<Vec<ProjectionClaimVersion>, MemoryError> {
        self.with_read_conn(move |conn| projection_storage::query_claim_versions(conn, &query))
            .await
    }

    /// Query imported relation projection rows through the supported public read surface.
    pub async fn query_relation_versions(
        &self,
        query: ProjectionQuery,
    ) -> Result<Vec<ProjectionRelationVersion>, MemoryError> {
        self.with_read_conn(move |conn| projection_storage::query_relation_versions(conn, &query))
            .await
    }

    /// Query imported episode projection rows through the supported public read surface.
    pub async fn query_episodes(
        &self,
        query: ProjectionQuery,
    ) -> Result<Vec<ProjectionEpisode>, MemoryError> {
        self.with_read_conn(move |conn| projection_storage::query_episode_rows(conn, &query))
            .await
    }

    /// Query imported entity-alias rows through the supported public read surface.
    pub async fn query_entity_aliases(
        &self,
        query: ProjectionQuery,
    ) -> Result<Vec<ProjectionEntityAlias>, MemoryError> {
        self.with_read_conn(move |conn| projection_storage::query_entity_aliases(conn, &query))
            .await
    }

    /// Query imported evidence-reference rows through the supported public read surface.
    pub async fn query_evidence_refs(
        &self,
        query: ProjectionQuery,
    ) -> Result<Vec<ProjectionEvidenceRef>, MemoryError> {
        self.with_read_conn(move |conn| projection_storage::query_evidence_refs(conn, &query))
            .await
    }

    /// Execute raw SQL. For testing only — not part of the stable public API.
    #[cfg(any(test, feature = "testing"))]
    pub async fn raw_execute(&self, sql: &str, params: Vec<String>) -> Result<usize, MemoryError> {
        let sql = sql.to_string();
        self.with_write_conn(move |conn| {
            let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
                .iter()
                .map(|s| s as &dyn rusqlite::types::ToSql)
                .collect();
            Ok(conn.execute(&sql, &*param_refs)?)
        })
        .await
    }
}
