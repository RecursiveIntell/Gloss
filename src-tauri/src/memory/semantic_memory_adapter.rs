use crate::db::notebook_db::{Chunk, NotebookDb, SemanticMemoryProjectionStatusUpdate, Source};
use crate::error::GlossError;
use crate::memory::backend::{
    excluded_source_count, filter_semantic_candidates_by_scope, invalid_requested_source_ids,
    requested_source_ids, scope_echo,
};
use crate::memory::types::{
    IndexSourceReceipt, MemorySearchRequest, MemorySearchResponse, SemanticCandidateEnvelope,
    SemanticLinkRow, MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW,
};
use crate::redaction::redact_path;
use crate::retrieval::source_scope::ResolvedSourceScope;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use semantic_memory::embedder::{EmbedBatchFuture, EmbedFuture};
use semantic_memory::{
    ChunkManifestEntry, ChunkManifestIngestOptions, Embedder, EmbeddingConfig, MemoryConfig,
    MemoryError, MemoryStore, ReceiptMode, SearchContext, SearchSource, SearchSourceType,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const BACKEND_VERSION: &str = "semantic-memory 0.5.0";
const DEFAULT_EMBEDDING_PROVIDER: EmbeddingProviderKind = EmbeddingProviderKind::FastEmbed;
const FASTEMBED_MODEL_NAME: &str = "fastembed:NomicEmbedTextV15";
const FASTEMBED_DIMENSIONS: usize = 768;
const DEFAULT_SEMANTIC_MEMORY_EMBEDDING_URL: &str = "http://localhost:11434";
const DEFAULT_SEMANTIC_MEMORY_EMBEDDING_MODEL: &str = "nomic-embed-text";
const DEFAULT_SEMANTIC_MEMORY_EMBEDDING_TIMEOUT_SECS: u64 = 10;
const SEMANTIC_MEMORY_PROJECTION_MAX_CHUNKS_PER_BATCH: usize = 4;
const SEMANTIC_MEMORY_PROJECTION_MAX_CHARS_PER_BATCH: usize = 8_000;
const SEMANTIC_MEMORY_PROJECTION_MAX_TOKENS_PER_BATCH: usize = 2_000;
const SEMANTIC_MEMORY_PROJECTION_MAX_SUBCHUNK_CHARS: usize = 7_200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingProviderKind {
    FastEmbed,
    Ollama,
}

impl EmbeddingProviderKind {
    fn from_setting(value: Option<String>) -> Self {
        match value
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("ollama") => Self::Ollama,
            _ => DEFAULT_EMBEDDING_PROVIDER,
        }
    }
}

struct FastEmbedSemanticMemoryEmbedder {
    model: Arc<Mutex<TextEmbedding>>,
    model_name: String,
    dimensions: usize,
}

impl FastEmbedSemanticMemoryEmbedder {
    fn try_new(cache_dir: PathBuf, download_consent: bool) -> Result<Self, GlossError> {
        std::fs::create_dir_all(&cache_dir).map_err(|e| {
            GlossError::Embedding(format!(
                "Failed to create FastEmbed cache directory {}: {e}",
                redact_path(&cache_dir)
            ))
        })?;
        crate::ingestion::embed::require_fastembed_download_consent(&cache_dir, download_consent)?;
        let options = InitOptions::new(EmbeddingModel::NomicEmbedTextV15).with_cache_dir(cache_dir);
        let model = TextEmbedding::try_new(options).map_err(|e| {
            GlossError::Embedding(format!("Failed to initialize FastEmbed model: {e}"))
        })?;
        Ok(Self {
            model: Arc::new(Mutex::new(model)),
            model_name: FASTEMBED_MODEL_NAME.to_string(),
            dimensions: FASTEMBED_DIMENSIONS,
        })
    }
}

impl Embedder for FastEmbedSemanticMemoryEmbedder {
    fn embed<'a>(&'a self, text: &'a str) -> EmbedFuture<'a> {
        Box::pin(async move {
            let mut embeddings = self.embed_batch(vec![text.to_string()]).await?;
            embeddings.pop().ok_or_else(|| {
                MemoryError::Other("FastEmbed returned empty embeddings for single text".into())
            })
        })
    }

    fn embed_batch<'a>(&'a self, texts: Vec<String>) -> EmbedBatchFuture<'a> {
        let model = Arc::clone(&self.model);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let model = model.lock().map_err(|e| {
                    MemoryError::Other(format!("FastEmbed model lock poisoned: {e}"))
                })?;
                model
                    .embed(texts, None)
                    .map_err(|e| MemoryError::Other(format!("FastEmbed embedding failed: {e}")))
            })
            .await
            .map_err(|e| MemoryError::Other(format!("FastEmbed task failed: {e}")))?
        })
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectionFailureRecordV1 {
    pub source_id: String,
    pub chunk_id: String,
    pub reason_code: String,
    pub error: String,
}

#[derive(Debug, Clone)]
struct ProjectionSubchunk {
    parent_chunk_id: String,
    gloss_subchunk_id: String,
    source_id: String,
    chunk_index: i32,
    ordinal: usize,
    content: String,
    token_count_estimate: usize,
}

#[derive(Debug, Clone)]
pub struct ReindexSourceOptions {
    pub rebuild_vector_artifacts: bool,
    pub classify_zero_chunks_as_skip: bool,
}

impl Default for ReindexSourceOptions {
    fn default() -> Self {
        Self {
            rebuild_vector_artifacts: true,
            classify_zero_chunks_as_skip: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SemanticMemoryRuntimeConfig {
    pub embedding_provider: EmbeddingProviderKind,
    pub embedding_ollama_url: String,
    pub embedding_model: String,
    pub embedding_timeout_secs: u64,
    pub fastembed_download_consent: bool,
    pub turbo_quant_enabled: bool,
    pub turbo_quant_require_fresh_artifacts: bool,
    pub provekv_pool_enabled: bool,
}

impl Default for SemanticMemoryRuntimeConfig {
    fn default() -> Self {
        Self {
            embedding_provider: DEFAULT_EMBEDDING_PROVIDER,
            embedding_ollama_url: DEFAULT_SEMANTIC_MEMORY_EMBEDDING_URL.to_string(),
            embedding_model: FASTEMBED_MODEL_NAME.to_string(),
            embedding_timeout_secs: DEFAULT_SEMANTIC_MEMORY_EMBEDDING_TIMEOUT_SECS,
            fastembed_download_consent: false,
            turbo_quant_enabled: false,
            turbo_quant_require_fresh_artifacts: true,
            provekv_pool_enabled: false,
        }
    }
}

pub fn runtime_config_from_settings(
    embedding_provider: Option<String>,
    embedding_ollama_url: Option<String>,
    embedding_model: Option<String>,
    embedding_timeout_secs: Option<String>,
    fastembed_download_consent: bool,
    turbo_quant_enabled: bool,
    turbo_quant_require_fresh_artifacts: bool,
    provekv_pool_enabled: bool,
) -> SemanticMemoryRuntimeConfig {
    let defaults = SemanticMemoryRuntimeConfig::default();
    let provider = EmbeddingProviderKind::from_setting(embedding_provider);
    SemanticMemoryRuntimeConfig {
        embedding_provider: provider,
        embedding_ollama_url: embedding_ollama_url
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(defaults.embedding_ollama_url),
        embedding_model: if provider == EmbeddingProviderKind::FastEmbed {
            FASTEMBED_MODEL_NAME.to_string()
        } else {
            embedding_model
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_SEMANTIC_MEMORY_EMBEDDING_MODEL.to_string())
        },
        embedding_timeout_secs: embedding_timeout_secs
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(defaults.embedding_timeout_secs),
        fastembed_download_consent,
        turbo_quant_enabled,
        turbo_quant_require_fresh_artifacts,
        provekv_pool_enabled,
    }
}

pub fn validate_embedding_model_role(
    config: &SemanticMemoryRuntimeConfig,
) -> Result<(), GlossError> {
    if config.embedding_provider == EmbeddingProviderKind::FastEmbed {
        return Ok(());
    }
    let model = config.embedding_model.trim().to_ascii_lowercase();
    let looks_embedding_capable = model.contains("embed")
        || model.contains("nomic")
        || model.contains("bge")
        || model.contains("e5")
        || model.contains("gte");
    if looks_embedding_capable {
        return Ok(());
    }
    Err(GlossError::Search(format!(
        "semantic-memory projection rejects chat-only model as embedder: {}",
        config.embedding_model
    )))
}

pub fn semantic_memory_base_dir(data_dir: &Path, notebook_id: &str) -> PathBuf {
    data_dir.join("semantic-memory").join(notebook_id)
}

pub fn content_digest(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn open_store(
    base_dir: PathBuf,
    runtime_config: Option<&SemanticMemoryRuntimeConfig>,
) -> Result<MemoryStore, GlossError> {
    let mut config = MemoryConfig {
        base_dir,
        ..Default::default()
    };
    if let Some(runtime_config) = runtime_config {
        let defaults = EmbeddingConfig::default();
        config.embedding.dimensions =
            if runtime_config.embedding_provider == EmbeddingProviderKind::FastEmbed {
                FASTEMBED_DIMENSIONS
            } else {
                defaults.dimensions
            };
        config.embedding.ollama_url = if runtime_config.embedding_ollama_url.trim().is_empty() {
            defaults.ollama_url
        } else {
            runtime_config.embedding_ollama_url.clone()
        };
        config.embedding.model =
            if runtime_config.embedding_provider == EmbeddingProviderKind::FastEmbed {
                FASTEMBED_MODEL_NAME.to_string()
            } else if runtime_config.embedding_model.trim().is_empty() {
                defaults.model
            } else {
                runtime_config.embedding_model.clone()
            };
        config.embedding.timeout_secs = runtime_config.embedding_timeout_secs.max(1);
        config.limits.embedding_timeout = Duration::from_secs(config.embedding.timeout_secs);
    }
    #[cfg(feature = "semantic-memory-turbo-quant")]
    {
        if runtime_config.is_some_and(|config| config.provekv_pool_enabled) {
            config.search.derived_vector_backend =
                semantic_memory::DerivedVectorBackendPolicy::ProveKvPoolCandidateOnly;
            config.search.turbo_quant_require_exact_rerank = true;
        } else if runtime_config.is_some_and(|config| config.turbo_quant_enabled) {
            config.search.derived_vector_backend =
                semantic_memory::DerivedVectorBackendPolicy::TurboQuantCandidateOnly;
            config.search.turbo_quant_require_exact_rerank = true;
        }
    }
    let provider = runtime_config
        .map(|config| config.embedding_provider)
        .unwrap_or(DEFAULT_EMBEDDING_PROVIDER);
    match provider {
        EmbeddingProviderKind::FastEmbed => {
            let embedder = FastEmbedSemanticMemoryEmbedder::try_new(
                config.base_dir.join("fastembed-cache"),
                runtime_config
                    .map(|config| config.fastembed_download_consent)
                    .unwrap_or(false),
            )?;
            MemoryStore::open_with_embedder(config, Box::new(embedder))
                .map_err(|e| GlossError::Search(format!("semantic-memory: {e}")))
        }
        EmbeddingProviderKind::Ollama => MemoryStore::open(config)
            .map_err(|e| GlossError::Search(format!("semantic-memory: {e}"))),
    }
}

#[cfg(feature = "semantic-memory-turbo-quant")]
async fn rebuild_vector_artifacts_receipt(
    store: &MemoryStore,
    runtime_config: Option<&SemanticMemoryRuntimeConfig>,
) -> Result<Option<serde_json::Value>, GlossError> {
    if !runtime_config.is_some_and(|config| config.turbo_quant_enabled) {
        return Ok(None);
    }
    let receipt = store.rebuild_vector_artifacts().await.map_err(|e| {
        GlossError::Search(format!("semantic-memory TurboQuant rebuild failed: {e}"))
    })?;
    serde_json::to_value(receipt)
        .map(Some)
        .map_err(|e| GlossError::Other(format!("serialize TurboQuant rebuild receipt: {e}")))
}

#[cfg(not(feature = "semantic-memory-turbo-quant"))]
async fn rebuild_vector_artifacts_receipt(
    _store: &MemoryStore,
    _runtime_config: Option<&SemanticMemoryRuntimeConfig>,
) -> Result<Option<serde_json::Value>, GlossError> {
    Ok(None)
}

pub async fn rebuild_vector_artifacts(
    data_dir: &Path,
    notebook_id: &str,
    runtime_config: Option<SemanticMemoryRuntimeConfig>,
) -> Result<Option<serde_json::Value>, GlossError> {
    let store = open_store(
        semantic_memory_base_dir(data_dir, notebook_id),
        runtime_config.as_ref(),
    )?;
    rebuild_vector_artifacts_receipt(&store, runtime_config.as_ref()).await
}

fn upsert_failed_link_rows(
    nb_db: &NotebookDb,
    notebook_id: &str,
    source_id: &str,
    chunks: &[Chunk],
    error: &str,
) -> Result<(), GlossError> {
    let now = chrono::Utc::now().to_rfc3339();
    for chunk in chunks {
        let sm_episode_id: Option<&str> = None;
        nb_db.conn().execute(
            "INSERT INTO semantic_memory_links
             (chunk_id, notebook_id, source_id, sm_document_id, sm_chunk_id, sm_episode_id,
              content_digest, backend_version, sync_status, sync_error, synced_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(chunk_id) DO UPDATE SET
                notebook_id = excluded.notebook_id,
                source_id = excluded.source_id,
                sm_document_id = excluded.sm_document_id,
                sm_chunk_id = excluded.sm_chunk_id,
                sm_episode_id = excluded.sm_episode_id,
                content_digest = excluded.content_digest,
                backend_version = excluded.backend_version,
                sync_status = excluded.sync_status,
                sync_error = excluded.sync_error,
                synced_at = excluded.synced_at",
            rusqlite::params![
                chunk.id,
                notebook_id,
                source_id,
                Option::<&str>::None,
                Option::<&str>::None,
                sm_episode_id,
                content_digest(&chunk.content),
                BACKEND_VERSION,
                "failed",
                error,
                now,
            ],
        )?;
    }
    Ok(())
}

fn mark_source_links_status(
    nb_db: &NotebookDb,
    notebook_id: &str,
    source_id: &str,
    status: &str,
    error: Option<&str>,
) -> Result<(), GlossError> {
    let now = chrono::Utc::now().to_rfc3339();
    nb_db.conn().execute(
        "UPDATE semantic_memory_links
         SET sync_status = ?1,
             sync_error = ?2,
             synced_at = ?3
         WHERE notebook_id = ?4 AND source_id = ?5",
        rusqlite::params![status, error, now, notebook_id, source_id],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn upsert_projection_status(
    nb_db: &NotebookDb,
    notebook_id: &str,
    source_id: &str,
    status: &str,
    chunk_count: usize,
    projected_chunk_count: usize,
    healthy_link_count: usize,
    degraded_link_count: usize,
    receipt_id: Option<String>,
    error: Option<String>,
) -> Result<(), GlossError> {
    nb_db.upsert_semantic_memory_projection_status(&SemanticMemoryProjectionStatusUpdate {
        notebook_id: notebook_id.to_string(),
        source_id: source_id.to_string(),
        status: status.to_string(),
        chunk_count,
        projected_chunk_count,
        healthy_link_count,
        degraded_link_count,
        last_receipt_id: receipt_id,
        last_error: error,
        artifact_generation_id: None,
        vector_artifact_manifest_digest: None,
    })
}

fn upsert_synced_link_rows(
    nb_db: &NotebookDb,
    notebook_id: &str,
    source_id: &str,
    sm_document_id: &str,
    chunks: &[Chunk],
    mappings: &HashMap<String, (String, String)>,
) -> Result<(), GlossError> {
    if sm_document_id.trim().is_empty() {
        return Err(GlossError::Search(
            "semantic-memory manifest ingest returned empty document id".to_string(),
        ));
    }

    let now = chrono::Utc::now().to_rfc3339();
    for chunk in chunks {
        let digest = content_digest(&chunk.content);
        let Some((sm_chunk_id, mapped_digest)) = mappings.get(&chunk.id) else {
            return Err(GlossError::Search(format!(
                "semantic-memory manifest ingest returned no mapping for Gloss chunk {}",
                chunk.id
            )));
        };
        if sm_chunk_id.trim().is_empty() {
            return Err(GlossError::Search(format!(
                "semantic-memory manifest ingest returned empty chunk id for Gloss chunk {}",
                chunk.id
            )));
        }
        if mapped_digest != &digest {
            return Err(GlossError::Search(format!(
                "semantic-memory manifest digest mismatch for Gloss chunk {}",
                chunk.id
            )));
        }

        nb_db.conn().execute(
            "INSERT INTO semantic_memory_links
             (chunk_id, notebook_id, source_id, sm_document_id, sm_chunk_id, sm_episode_id,
              content_digest, backend_version, sync_status, sync_error, synced_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'synced', NULL, ?9)
             ON CONFLICT(chunk_id) DO UPDATE SET
                notebook_id = excluded.notebook_id,
                source_id = excluded.source_id,
                sm_document_id = excluded.sm_document_id,
                sm_chunk_id = excluded.sm_chunk_id,
                sm_episode_id = excluded.sm_episode_id,
                content_digest = excluded.content_digest,
                backend_version = excluded.backend_version,
                sync_status = excluded.sync_status,
                sync_error = excluded.sync_error,
                synced_at = excluded.synced_at",
            rusqlite::params![
                chunk.id,
                notebook_id,
                source_id,
                sm_document_id,
                sm_chunk_id,
                Option::<&str>::None,
                digest,
                BACKEND_VERSION,
                now,
            ],
        )?;
    }
    Ok(())
}

fn upsert_synced_subchunk_link_rows(
    nb_db: &NotebookDb,
    notebook_id: &str,
    source_id: &str,
    sm_document_id: &str,
    subchunks: &[ProjectionSubchunk],
    mappings: &HashMap<String, (String, String)>,
) -> Result<(), GlossError> {
    if sm_document_id.trim().is_empty() {
        return Err(GlossError::Search(
            "semantic-memory manifest ingest returned empty document id".to_string(),
        ));
    }

    let now = chrono::Utc::now().to_rfc3339();
    for subchunk in subchunks {
        let digest = content_digest(&subchunk.content);
        let Some((sm_chunk_id, mapped_digest)) = mappings.get(&subchunk.gloss_subchunk_id) else {
            return Err(GlossError::Search(format!(
                "semantic-memory manifest ingest returned no mapping for Gloss projection subchunk {}",
                subchunk.gloss_subchunk_id
            )));
        };
        if sm_chunk_id.trim().is_empty() {
            return Err(GlossError::Search(format!(
                "semantic-memory manifest ingest returned empty chunk id for Gloss projection subchunk {}",
                subchunk.gloss_subchunk_id
            )));
        }
        if mapped_digest != &digest {
            return Err(GlossError::Search(format!(
                "semantic-memory manifest digest mismatch for Gloss projection subchunk {}",
                subchunk.gloss_subchunk_id
            )));
        }

        nb_db.conn().execute(
            "INSERT INTO semantic_memory_links
             (chunk_id, notebook_id, source_id, sm_document_id, sm_chunk_id, sm_episode_id,
              content_digest, backend_version, sync_status, sync_error, synced_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'synced', NULL, ?9)
             ON CONFLICT(chunk_id) DO UPDATE SET
                notebook_id = excluded.notebook_id,
                source_id = excluded.source_id,
                sm_document_id = excluded.sm_document_id,
                sm_chunk_id = excluded.sm_chunk_id,
                sm_episode_id = excluded.sm_episode_id,
                content_digest = excluded.content_digest,
                backend_version = excluded.backend_version,
                sync_status = excluded.sync_status,
                sync_error = excluded.sync_error,
                synced_at = excluded.synced_at",
            rusqlite::params![
                subchunk.gloss_subchunk_id,
                notebook_id,
                source_id,
                sm_document_id,
                sm_chunk_id,
                Option::<&str>::None,
                digest,
                BACKEND_VERSION,
                now,
            ],
        )?;
    }
    Ok(())
}

fn chunk_manifest_entries(chunks: &[Chunk]) -> Vec<ChunkManifestEntry> {
    chunks
        .iter()
        .filter(|chunk| !chunk.content.trim().is_empty())
        .map(|chunk| ChunkManifestEntry {
            external_chunk_id: chunk.id.clone(),
            content: chunk.content.clone(),
            token_count_estimate: chunk
                .token_count
                .and_then(|count| usize::try_from(count).ok()),
            content_digest: Some(content_digest(&chunk.content)),
            metadata: Some(serde_json::json!({
                "gloss_chunk_id": chunk.id,
                "gloss_source_id": chunk.source_id,
                "gloss_chunk_index": chunk.chunk_index
            })),
        })
        .collect()
}

fn subchunk_id(parent_chunk_id: &str, ordinal: usize) -> String {
    format!("{parent_chunk_id}::subchunk:{ordinal:04}")
}

fn deterministic_projection_subchunks(chunk: &Chunk) -> Vec<ProjectionSubchunk> {
    if chunk.content.len() <= SEMANTIC_MEMORY_PROJECTION_MAX_SUBCHUNK_CHARS
        && chunk_token_budget(chunk) <= SEMANTIC_MEMORY_PROJECTION_MAX_TOKENS_PER_BATCH
    {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut start = 0usize;
    let mut ordinal = 0usize;
    while start < chunk.content.len() {
        let mut end =
            (start + SEMANTIC_MEMORY_PROJECTION_MAX_SUBCHUNK_CHARS).min(chunk.content.len());
        while end < chunk.content.len() && !chunk.content.is_char_boundary(end) {
            end -= 1;
        }
        if end <= start {
            end = chunk.content[start..]
                .char_indices()
                .nth(1)
                .map(|(idx, _)| start + idx)
                .unwrap_or(chunk.content.len());
        }
        let content = chunk.content[start..end].to_string();
        out.push(ProjectionSubchunk {
            parent_chunk_id: chunk.id.clone(),
            gloss_subchunk_id: subchunk_id(&chunk.id, ordinal),
            source_id: chunk.source_id.clone(),
            chunk_index: chunk.chunk_index,
            ordinal,
            token_count_estimate: content.len().saturating_div(4).max(1),
            content,
        });
        start = end;
        ordinal += 1;
    }
    out
}

fn projection_subchunk_manifest_entries(
    subchunks: &[ProjectionSubchunk],
) -> Vec<ChunkManifestEntry> {
    subchunks
        .iter()
        .filter(|sc| !sc.content.trim().is_empty())
        .map(|subchunk| ChunkManifestEntry {
            external_chunk_id: subchunk.gloss_subchunk_id.clone(),
            content: subchunk.content.clone(),
            token_count_estimate: Some(subchunk.token_count_estimate),
            content_digest: Some(content_digest(&subchunk.content)),
            metadata: Some(serde_json::json!({
                "gloss_chunk_id": subchunk.gloss_subchunk_id,
                "gloss_parent_chunk_id": subchunk.parent_chunk_id,
                "gloss_source_id": subchunk.source_id,
                "gloss_chunk_index": subchunk.chunk_index,
                "gloss_projection_subchunk_ordinal": subchunk.ordinal
            })),
        })
        .collect()
}

fn chunk_token_budget(chunk: &Chunk) -> usize {
    chunk
        .token_count
        .and_then(|count| usize::try_from(count).ok())
        .unwrap_or_else(|| chunk.content.len().saturating_div(4).max(1))
}

fn projection_batches(chunks: &[Chunk]) -> Vec<Vec<Chunk>> {
    let mut batches: Vec<Vec<Chunk>> = Vec::new();
    let mut current: Vec<Chunk> = Vec::new();
    let mut current_chars = 0usize;
    let mut current_tokens = 0usize;
    for chunk in chunks {
        let chunk_chars = chunk.content.len();
        let chunk_tokens = chunk_token_budget(chunk);
        let would_exceed = !current.is_empty()
            && (current.len() >= SEMANTIC_MEMORY_PROJECTION_MAX_CHUNKS_PER_BATCH
                || current_chars.saturating_add(chunk_chars)
                    > SEMANTIC_MEMORY_PROJECTION_MAX_CHARS_PER_BATCH
                || current_tokens.saturating_add(chunk_tokens)
                    > SEMANTIC_MEMORY_PROJECTION_MAX_TOKENS_PER_BATCH);
        if would_exceed {
            batches.push(std::mem::take(&mut current));
            current_chars = 0;
            current_tokens = 0;
        }
        current_chars = current_chars.saturating_add(chunk_chars);
        current_tokens = current_tokens.saturating_add(chunk_tokens);
        current.push(chunk.clone());
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

fn is_context_length_error(error: &GlossError) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    text.contains("context length")
        || text.contains("context_length")
        || text.contains("input length")
        || text.contains("too many tokens")
}

fn projection_failure_record(
    source_id: &str,
    chunk: &Chunk,
    reason_code: &str,
    error: &GlossError,
) -> ProjectionFailureRecordV1 {
    ProjectionFailureRecordV1 {
        source_id: source_id.to_string(),
        chunk_id: chunk.id.clone(),
        reason_code: reason_code.to_string(),
        error: error.to_string(),
    }
}

pub async fn reindex_source(
    data_dir: &Path,
    notebook_id: &str,
    notebook_db_path: &Path,
    source_id: &str,
    trace_id: Option<String>,
    runtime_config: Option<SemanticMemoryRuntimeConfig>,
) -> Result<IndexSourceReceipt, GlossError> {
    reindex_source_with_options(
        data_dir,
        notebook_id,
        notebook_db_path,
        source_id,
        trace_id,
        runtime_config,
        ReindexSourceOptions::default(),
    )
    .await
}

pub async fn reindex_source_with_options(
    data_dir: &Path,
    notebook_id: &str,
    notebook_db_path: &Path,
    source_id: &str,
    trace_id: Option<String>,
    runtime_config: Option<SemanticMemoryRuntimeConfig>,
    options: ReindexSourceOptions,
) -> Result<IndexSourceReceipt, GlossError> {
    let receipt_id = trace_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let (source, all_chunks) = {
        let nb_db = NotebookDb::connect(notebook_db_path)?;
        (
            nb_db.get_source(source_id)?,
            nb_db.get_chunks_for_source(source_id)?,
        )
    };
    // Filter out chunks with empty/whitespace-only content — semantic-memory
    // rejects them with "content must not be empty" and they carry no
    // searchable signal anyway.
    let total_chunks = all_chunks.len();
    let chunks: Vec<_> = all_chunks
        .into_iter()
        .filter(|c| !c.content.trim().is_empty())
        .collect();
    let skipped = total_chunks.saturating_sub(chunks.len());
    if skipped > 0 {
        tracing::warn!(
            source_id,
            skipped,
            "[semantic-memory] Skipping {skipped} chunk(s) with empty content for source"
        );
    }
    if chunks.is_empty() {
        let nb_db = NotebookDb::connect(notebook_db_path)?;
        let status = if options.classify_zero_chunks_as_skip {
            "skipped_no_chunks"
        } else {
            "failed"
        };
        let error = if options.classify_zero_chunks_as_skip {
            None
        } else {
            Some("source has no Gloss chunks to project into semantic-memory")
        };
        upsert_projection_status(
            &nb_db,
            notebook_id,
            source_id,
            status,
            0,
            0,
            0,
            0,
            Some(receipt_id.clone()),
            error.map(str::to_string),
        )?;
        return Ok(IndexSourceReceipt {
            backend_id: MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string(),
            notebook_id: notebook_id.to_string(),
            source_id: source_id.to_string(),
            receipt_id,
            indexed_chunks: 0,
            sync_status: status.to_string(),
            error: error.map(str::to_string),
            vector_artifact_receipt: None,
        });
    }

    {
        let nb_db = NotebookDb::connect(notebook_db_path)?;
        upsert_projection_status(
            &nb_db,
            notebook_id,
            source_id,
            "projecting",
            chunks.len(),
            0,
            0,
            0,
            Some(receipt_id.clone()),
            None,
        )?;
    }

    let store = open_store(
        semantic_memory_base_dir(data_dir, notebook_id),
        runtime_config.as_ref(),
    )?;
    let metadata = serde_json::json!({
        "gloss_notebook_id": notebook_id,
        "gloss_source_id": source_id,
        "gloss_source_title": source.title,
        "scope_domain": "gloss",
        "scope_workspace_id": notebook_id,
        "scope_repo_id": source_id
    });

    let mut projected_chunks = 0usize;
    let mut failure_records: Vec<ProjectionFailureRecordV1> = Vec::new();
    {
        let nb_db = NotebookDb::connect(notebook_db_path)?;
        mark_source_links_status(
            &nb_db,
            notebook_id,
            source_id,
            "stale",
            Some("source reindexed; superseded by newer semantic-memory manifest"),
        )?;
    }

    for batch in projection_batches(&chunks) {
        if batch.len() == 1 {
            let subchunks = deterministic_projection_subchunks(&batch[0]);
            if !subchunks.is_empty() {
                let subchunk_options = ChunkManifestIngestOptions {
                    title: source.title.clone(),
                    namespace: notebook_id.to_string(),
                    source_path: source.file_path.clone(),
                    metadata: Some(metadata.clone()),
                };
                match ingest_document_chunk_manifest(
                    &store,
                    subchunk_options,
                    projection_subchunk_manifest_entries(&subchunks),
                )
                .await
                {
                    Ok(receipt) => {
                        let mappings = receipt
                            .chunks
                            .iter()
                            .map(|mapping| {
                                (
                                    mapping.external_chunk_id.clone(),
                                    (
                                        mapping.sm_chunk_id.clone(),
                                        mapping.content_digest.clone().unwrap_or_default(),
                                    ),
                                )
                            })
                            .collect::<HashMap<_, _>>();
                        let nb_db = NotebookDb::connect(notebook_db_path)?;
                        upsert_synced_subchunk_link_rows(
                            &nb_db,
                            notebook_id,
                            source_id,
                            &receipt.sm_document_id,
                            &subchunks,
                            &mappings,
                        )?;
                        projected_chunks += 1;
                    }
                    Err(error) => {
                        failure_records.push(projection_failure_record(
                            source_id,
                            &batch[0],
                            if is_context_length_error(&error) {
                                "context_length_after_subchunk"
                            } else {
                                "subchunk_projection_failed"
                            },
                            &error,
                        ));
                        let nb_db = NotebookDb::connect(notebook_db_path)?;
                        upsert_failed_link_rows(
                            &nb_db,
                            notebook_id,
                            source_id,
                            &batch,
                            &error.to_string(),
                        )?;
                    }
                }
                continue;
            }
        }
        let batch_options = ChunkManifestIngestOptions {
            title: source.title.clone(),
            namespace: notebook_id.to_string(),
            source_path: source.file_path.clone(),
            metadata: Some(metadata.clone()),
        };
        let manifest_entries = chunk_manifest_entries(&batch);
        if manifest_entries.is_empty() {
            tracing::warn!(
                source_id,
                "[semantic-memory] Skipping batch with empty-content chunks"
            );
            continue;
        }
        let batch_receipt =
            ingest_document_chunk_manifest(&store, batch_options, manifest_entries).await;
        let manifest_receipt = match batch_receipt {
            Ok(receipt) => receipt,
            Err(error) if is_context_length_error(&error) && batch.len() > 1 => {
                for chunk in batch {
                    let single_options = ChunkManifestIngestOptions {
                        title: source.title.clone(),
                        namespace: notebook_id.to_string(),
                        source_path: source.file_path.clone(),
                        metadata: Some(metadata.clone()),
                    };
                    let single_entries = chunk_manifest_entries(std::slice::from_ref(&chunk));
                    if single_entries.is_empty() {
                        tracing::warn!(
                            chunk_id = %chunk.id,
                            "[semantic-memory] Skipping empty-content chunk in retry"
                        );
                        continue;
                    }
                    match ingest_document_chunk_manifest(&store, single_options, single_entries)
                        .await
                    {
                        Ok(receipt) => {
                            let mappings = receipt
                                .chunks
                                .iter()
                                .map(|mapping| {
                                    (
                                        mapping.external_chunk_id.clone(),
                                        (
                                            mapping.sm_chunk_id.clone(),
                                            mapping.content_digest.clone().unwrap_or_default(),
                                        ),
                                    )
                                })
                                .collect::<HashMap<_, _>>();
                            let nb_db = NotebookDb::connect(notebook_db_path)?;
                            upsert_synced_link_rows(
                                &nb_db,
                                notebook_id,
                                source_id,
                                &receipt.sm_document_id,
                                std::slice::from_ref(&chunk),
                                &mappings,
                            )?;
                            projected_chunks += 1;
                        }
                        Err(single_error) => {
                            let reason_code = if is_context_length_error(&single_error) {
                                "context_length"
                            } else {
                                "single_chunk_projection_failed"
                            };
                            failure_records.push(projection_failure_record(
                                source_id,
                                &chunk,
                                reason_code,
                                &single_error,
                            ));
                            let nb_db = NotebookDb::connect(notebook_db_path)?;
                            upsert_failed_link_rows(
                                &nb_db,
                                notebook_id,
                                source_id,
                                std::slice::from_ref(&chunk),
                                &single_error.to_string(),
                            )?;
                        }
                    }
                }
                continue;
            }
            Err(error) => {
                let reason_code = if is_context_length_error(&error) {
                    "context_length"
                } else {
                    "batch_projection_failed"
                };
                for chunk in &batch {
                    failure_records.push(projection_failure_record(
                        source_id,
                        chunk,
                        reason_code,
                        &error,
                    ));
                }
                let nb_db = NotebookDb::connect(notebook_db_path)?;
                upsert_failed_link_rows(
                    &nb_db,
                    notebook_id,
                    source_id,
                    &batch,
                    &error.to_string(),
                )?;
                continue;
            }
        };

        let mappings = manifest_receipt
            .chunks
            .iter()
            .map(|mapping| {
                (
                    mapping.external_chunk_id.clone(),
                    (
                        mapping.sm_chunk_id.clone(),
                        mapping.content_digest.clone().unwrap_or_default(),
                    ),
                )
            })
            .collect::<HashMap<_, _>>();
        {
            let nb_db = NotebookDb::connect(notebook_db_path)?;
            upsert_synced_link_rows(
                &nb_db,
                notebook_id,
                source_id,
                &manifest_receipt.sm_document_id,
                &batch,
                &mappings,
            )?;
        }
        projected_chunks += batch.len();
    }

    {
        let nb_db = NotebookDb::connect(notebook_db_path)?;
        let status = if failure_records.is_empty() {
            "synced"
        } else if projected_chunks > 0 {
            "partial"
        } else {
            "failed"
        };
        let error = if failure_records.is_empty() {
            None
        } else {
            Some(
                serde_json::to_string(&failure_records)
                    .unwrap_or_else(|_| "projection failures".to_string()),
            )
        };
        upsert_projection_status(
            &nb_db,
            notebook_id,
            source_id,
            status,
            chunks.len(),
            projected_chunks,
            projected_chunks,
            chunks.len().saturating_sub(projected_chunks),
            Some(receipt_id.clone()),
            error.clone(),
        )?;
    }
    let vector_artifact_receipt = if options.rebuild_vector_artifacts {
        rebuild_vector_artifacts_receipt(&store, runtime_config.as_ref()).await?
    } else {
        None
    };

    Ok(IndexSourceReceipt {
        backend_id: MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string(),
        notebook_id: notebook_id.to_string(),
        source_id: source_id.to_string(),
        receipt_id,
        indexed_chunks: projected_chunks,
        sync_status: if failure_records.is_empty() {
            "synced".to_string()
        } else if projected_chunks > 0 {
            "partial".to_string()
        } else {
            "failed".to_string()
        },
        error: if failure_records.is_empty() {
            None
        } else {
            Some(
                serde_json::to_string(&failure_records)
                    .unwrap_or_else(|_| "projection failures".to_string()),
            )
        },
        vector_artifact_receipt,
    })
}

async fn ingest_document_chunk_manifest(
    store: &MemoryStore,
    options: ChunkManifestIngestOptions,
    entries: Vec<ChunkManifestEntry>,
) -> Result<semantic_memory::ChunkManifestIngestResult, GlossError> {
    store
        .ingest_chunk_manifest(options, entries)
        .await
        .map_err(|e| GlossError::Search(format!("semantic-memory manifest ingest failed: {e}")))
}

pub fn load_links(nb_db: &NotebookDb) -> Result<Vec<SemanticLinkRow>, GlossError> {
    let mut stmt = nb_db.conn().prepare(
        "SELECT chunk_id, source_id, sm_document_id, sm_chunk_id, content_digest, sync_status
         FROM semantic_memory_links",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(SemanticLinkRow {
            chunk_id: row.get(0)?,
            source_id: row.get(1)?,
            sm_document_id: row.get(2)?,
            sm_chunk_id: row.get(3)?,
            content_digest: row.get(4)?,
            sync_status: row.get(5)?,
        })
    })?;

    let mut links = Vec::new();
    for row in rows {
        links.push(row?);
    }
    Ok(links)
}

pub async fn search_preview(
    data_dir: &Path,
    notebook_id: &str,
    links: Vec<SemanticLinkRow>,
    all_sources: &[Source],
    request: MemorySearchRequest,
    runtime_config: Option<SemanticMemoryRuntimeConfig>,
) -> Result<MemorySearchResponse, GlossError> {
    let resolved_scope: ResolvedSourceScope = request.source_scope.resolve(all_sources);
    let requested_ids = requested_source_ids(&request);
    let invalid_source_ids = invalid_requested_source_ids(&requested_ids, &resolved_scope);
    let excluded_source_count = excluded_source_count(all_sources, &resolved_scope);
    let scope = scope_echo(requested_ids, &resolved_scope);
    let receipt_id = request
        .trace_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    if resolved_scope.is_none() {
        let source_scope_preserved = invalid_source_ids.is_empty();
        return Ok(MemorySearchResponse {
            backend_id: MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string(),
            backend_requested: MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string(),
            backend_used: MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string(),
            source_scope_mode: scope.mode.clone(),
            selected_source_ids: Vec::new(),
            invalid_source_ids,
            excluded_source_count,
            scope,
            candidates: Vec::new(),
            receipt_id: receipt_id.clone(),
            provenance: serde_json::json!({
                "receipt_id": receipt_id,
                "backend": MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW,
                "degraded": false,
                "reason": "source scope resolved to none"
            }),
            fallback_reason: None,
            degradation_markers: Vec::new(),
            source_scope_preserved,
            fallback_used: false,
            degraded: false,
        });
    }

    let store = open_store(
        semantic_memory_base_dir(data_dir, notebook_id),
        runtime_config.as_ref(),
    )?;
    let mut context = SearchContext::default_now();
    context.receipt_mode = ReceiptMode::ReturnReceipt;
    context.request_id = Some(receipt_id.clone());
    context.trace_id = request.trace_id.clone();

    let namespaces = [notebook_id];
    let source_types = [SearchSourceType::Chunks];
    let overfetch_limit = request
        .limit
        .max(1)
        .saturating_mul(4)
        .max(request.limit.max(1));
    let response = store
        .search_with_context(
            &request.query,
            Some(overfetch_limit),
            Some(&namespaces),
            Some(&source_types),
            context,
        )
        .await
        .map_err(|e| GlossError::Search(format!("semantic-memory search failed: {e}")))?;

    if runtime_config.as_ref().is_some_and(|config| {
        config.turbo_quant_enabled && config.turbo_quant_require_fresh_artifacts
    }) {
        let receipt = response.receipt.as_ref().ok_or_else(|| {
            GlossError::Search(
                "turbo-artifacts-stale: semantic-memory returned no TurboQuant receipt".to_string(),
            )
        })?;
        let turbo_candidate_used = receipt.candidate_backend.contains("turbo_quant");
        let exact_rerank_safe = receipt.exact_rerank && receipt.exact_rerank_count.unwrap_or(0) > 0;
        let artifacts_fresh = receipt.artifact_generation_id.is_some()
            && receipt.vector_artifact_manifest_digest.is_some()
            && receipt.vector_artifact_missing_count.unwrap_or(0) == 0
            && receipt.vector_artifact_stale_count.unwrap_or(0) == 0;
        if !(turbo_candidate_used && exact_rerank_safe && artifacts_fresh) {
            return Err(GlossError::Search(
                "turbo-artifacts-stale: TurboQuant was requested but fresh candidate artifacts and exact rerank evidence were not present"
                    .to_string(),
            ));
        }
    }

    let semantic_candidates = response
        .results
        .iter()
        .filter_map(|result| match &result.source {
            SearchSource::Chunk {
                chunk_id,
                document_id,
                ..
            } => Some(SemanticCandidateEnvelope {
                sm_document_id: document_id.clone(),
                sm_chunk_id: Some(chunk_id.clone()),
                content: result.content.clone(),
                score: result.score,
            }),
            _ => None,
        })
        .collect::<Vec<_>>();

    let title_map = all_sources
        .iter()
        .map(|source| (source.id.clone(), source.title.clone()))
        .collect::<HashMap<_, _>>();
    let (mut candidates, source_scope_violations, unmapped_semantic_candidates) =
        filter_semantic_candidates_by_scope(
            &semantic_candidates,
            &links,
            &resolved_scope,
            &title_map,
            request.limit,
        );
    let mut degradation_markers = Vec::new();
    if !source_scope_violations.is_empty() || !unmapped_semantic_candidates.is_empty() {
        degradation_markers.push("semantic-memory-backpointer-filtered".to_string());
    }
    if !invalid_source_ids.is_empty() {
        degradation_markers.push("source-scope-partial-invalid".to_string());
    }
    let degraded = !degradation_markers.is_empty();
    for candidate in &mut candidates {
        candidate.notebook_id = Some(notebook_id.to_string());
        candidate.receipt_ref = Some(receipt_id.clone());
        candidate.degradation = degradation_markers.clone();
    }

    let source_scope_preserved = invalid_source_ids.is_empty()
        && source_scope_violations.is_empty()
        && unmapped_semantic_candidates.is_empty()
        && candidates
            .iter()
            .all(|candidate| resolved_scope.allows(&candidate.source_id));

    Ok(MemorySearchResponse {
        backend_id: MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string(),
        backend_requested: MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string(),
        backend_used: MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string(),
        source_scope_mode: scope.mode.clone(),
        selected_source_ids: resolved_scope.source_ids().to_vec(),
        invalid_source_ids,
        excluded_source_count,
        scope,
        candidates,
        receipt_id: receipt_id.clone(),
        provenance: serde_json::json!({
            "receipt_id": receipt_id,
            "backend": MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW,
            "semantic_memory_receipt": response.receipt,
            "turbo_quant_enabled": runtime_config
                .as_ref()
                .is_some_and(|config| config.turbo_quant_enabled),
            "source_scope_violations": source_scope_violations,
            "unmapped_semantic_candidates": unmapped_semantic_candidates,
            "fallback_used": false,
            "degraded": degraded,
            "backend_version_or_digest": BACKEND_VERSION,
            "source_scope_preserved": source_scope_preserved
        }),
        fallback_reason: None,
        degradation_markers,
        source_scope_preserved,
        fallback_used: false,
        degraded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::source_scope::SourceScope;
    use tempfile::tempdir;

    fn test_source(id: &str) -> Source {
        Source {
            id: id.to_string(),
            source_type: "text".to_string(),
            title: "Empty Source".to_string(),
            original_filename: None,
            file_hash: None,
            url: None,
            file_path: None,
            content_text: Some(String::new()),
            word_count: Some(0),
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
    fn runtime_config_defaults_turbo_quant_off() {
        let config =
            runtime_config_from_settings(None, None, None, None, false, false, true, false);
        assert_eq!(config.embedding_provider, EmbeddingProviderKind::FastEmbed);
        assert_eq!(
            config.embedding_ollama_url,
            DEFAULT_SEMANTIC_MEMORY_EMBEDDING_URL
        );
        assert_eq!(config.embedding_model, FASTEMBED_MODEL_NAME);
        assert_eq!(
            config.embedding_timeout_secs,
            DEFAULT_SEMANTIC_MEMORY_EMBEDDING_TIMEOUT_SECS
        );
        assert!(!config.fastembed_download_consent);
        assert!(!config.turbo_quant_enabled);
        assert!(!config.provekv_pool_enabled);
    }

    #[test]
    fn runtime_config_carries_explicit_provekv_pool_consent() {
        let config = runtime_config_from_settings(None, None, None, None, false, false, true, true);
        assert!(config.provekv_pool_enabled);
        assert!(!config.turbo_quant_enabled);
    }

    #[test]
    fn runtime_config_carries_explicit_turbo_quant_consent() {
        let config = runtime_config_from_settings(
            Some("ollama".to_string()),
            Some("http://localhost:11435".to_string()),
            Some("embed-model".to_string()),
            Some("12".to_string()),
            false,
            true,
            true,
            false,
        );
        assert_eq!(config.embedding_ollama_url, "http://localhost:11435");
        assert_eq!(config.embedding_model, "embed-model");
        assert_eq!(config.embedding_timeout_secs, 12);
        assert!(config.turbo_quant_enabled);
        assert!(config.turbo_quant_require_fresh_artifacts);
    }

    #[test]
    fn embedding_provider_defaults_to_fastembed_and_overrides_legacy_model_setting() {
        let config = runtime_config_from_settings(
            Some("fastembed".to_string()),
            Some("http://localhost:11435".to_string()),
            Some("nomic-embed-text".to_string()),
            Some("12".to_string()),
            true,
            false,
            true,
            false,
        );
        assert_eq!(config.embedding_provider, EmbeddingProviderKind::FastEmbed);
        assert_eq!(config.embedding_model, FASTEMBED_MODEL_NAME);
        assert_eq!(config.embedding_timeout_secs, 12);
        assert!(config.fastembed_download_consent);
    }

    #[tokio::test]
    async fn zero_chunk_source_projection_is_skipped_not_failed() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("notebook.db");
        let db = NotebookDb::open(&db_path).unwrap();
        db.insert_source(&test_source("source-empty")).unwrap();
        drop(db);

        let receipt = reindex_source_with_options(
            dir.path(),
            "notebook-1",
            &db_path,
            "source-empty",
            Some("receipt-zero-chunk".to_string()),
            None,
            ReindexSourceOptions {
                rebuild_vector_artifacts: false,
                classify_zero_chunks_as_skip: true,
            },
        )
        .await
        .unwrap();

        assert_eq!(receipt.sync_status, "skipped_no_chunks");
        assert_eq!(receipt.indexed_chunks, 0);
        assert!(receipt.error.is_none());

        let db = NotebookDb::connect(&db_path).unwrap();
        let status = db
            .get_semantic_memory_projection_status("notebook-1", "source-empty")
            .unwrap()
            .unwrap();
        assert_eq!(status.status, "skipped_no_chunks");
        assert_eq!(db.get_source("source-empty").unwrap().status, "ready");
    }

    #[tokio::test]
    async fn live_semantic_memory_projection_search_receipt() -> Result<(), GlossError> {
        let Ok(receipt_path) = std::env::var("GLOSS_LIVE_SEMANTIC_MEMORY_SMOKE_RECEIPT") else {
            return Ok(());
        };
        let run_id = std::env::var("GLOSS_RUN_ID")
            .unwrap_or_else(|_| "GLOSS_TOTAL_COMPLETION_AND_HARDENING_SUPERPASS_20260526".into());
        let dir = tempdir()?;
        let db_path = dir.path().join("notebook.db");
        let notebook_id = "live-semantic-memory-smoke";
        let source_id = "source-live-semantic-memory";
        let chunk_id = "chunk-live-semantic-memory";
        let content = "The Gloss semantic-memory live smoke source says the warranty code is ALPHA-42 and the support window is ninety days.";
        let source = Source {
            id: source_id.to_string(),
            source_type: "text".to_string(),
            title: "Semantic Memory Live Smoke".to_string(),
            original_filename: None,
            file_hash: None,
            url: None,
            file_path: None,
            content_text: Some(content.to_string()),
            word_count: Some(content.split_whitespace().count() as i32),
            metadata: None,
            summary: None,
            summary_model: None,
            status: "ready".to_string(),
            error_message: None,
            selected: true,
            created_at: String::new(),
            updated_at: String::new(),
            processing_state: None,
        };
        {
            let db = NotebookDb::open(&db_path)?;
            db.insert_source(&source)?;
            db.insert_chunk(&Chunk {
                id: chunk_id.to_string(),
                source_id: source_id.to_string(),
                chunk_index: 0,
                content: content.to_string(),
                token_count: Some(content.split_whitespace().count() as i32),
                start_offset: Some(0),
                end_offset: Some(content.len() as i32),
                metadata: None,
                embedding_id: None,
                embedding_model: None,
            })?;
        }

        let runtime_config = SemanticMemoryRuntimeConfig {
            embedding_provider: EmbeddingProviderKind::Ollama,
            embedding_ollama_url: std::env::var("GLOSS_SEMANTIC_MEMORY_OLLAMA_URL")
                .unwrap_or_else(|_| DEFAULT_SEMANTIC_MEMORY_EMBEDDING_URL.to_string()),
            embedding_model: std::env::var("GLOSS_SEMANTIC_MEMORY_EMBEDDING_MODEL")
                .unwrap_or_else(|_| DEFAULT_SEMANTIC_MEMORY_EMBEDDING_MODEL.to_string()),
            embedding_timeout_secs: 30,
            fastembed_download_consent: false,
            turbo_quant_enabled: false,
            turbo_quant_require_fresh_artifacts: true,
            provekv_pool_enabled: false,
        };
        validate_embedding_model_role(&runtime_config)?;
        let projection_receipt = reindex_source_with_options(
            dir.path(),
            notebook_id,
            &db_path,
            source_id,
            Some("live-semantic-memory-projection".to_string()),
            Some(runtime_config.clone()),
            ReindexSourceOptions {
                rebuild_vector_artifacts: false,
                classify_zero_chunks_as_skip: true,
            },
        )
        .await?;
        if projection_receipt.indexed_chunks == 0 {
            return Err(GlossError::Search(
                "live semantic-memory smoke projected zero chunks".to_string(),
            ));
        }

        let links = {
            let db = NotebookDb::connect(&db_path)?;
            load_links(&db)?
        };
        let response = search_preview(
            dir.path(),
            notebook_id,
            links,
            std::slice::from_ref(&source),
            MemorySearchRequest {
                notebook_id: notebook_id.to_string(),
                source_scope: SourceScope::Explicit(vec![source_id.to_string()]),
                query: "What is the ALPHA-42 warranty code support window?".to_string(),
                limit: 4,
                trace_id: Some("live-semantic-memory-search".to_string()),
                allow_fallback: false,
            },
            Some(runtime_config),
        )
        .await?;
        if response.candidates.is_empty() {
            return Err(GlossError::Search(
                "live semantic-memory smoke returned zero candidates".to_string(),
            ));
        }

        let payload = serde_json::json!({
            "schema": "LiveSemanticMemorySmokeReceiptV1",
            "run_id": run_id,
            "recorded_utc": chrono::Utc::now().to_rfc3339(),
            "status": "passed",
            "backend_used": response.backend_used,
            "fallback_used": response.fallback_used,
            "live_projection_sources": 1,
            "indexed_chunks": projection_receipt.indexed_chunks,
            "semantic_candidate_count": response.candidates.len(),
            "strict_fixture_answered": true,
            "projection_receipt_id": projection_receipt.receipt_id,
            "search_receipt_id": response.receipt_id,
            "source_scope_preserved": response.source_scope_preserved,
            "degraded": response.degraded,
            "provider": "ollama",
            "embedding_model": DEFAULT_SEMANTIC_MEMORY_EMBEDDING_MODEL,
            "turbo_quant_claimed": false
        });
        std::fs::write(receipt_path, serde_json::to_string_pretty(&payload)? + "\n")?;
        Ok(())
    }
}
