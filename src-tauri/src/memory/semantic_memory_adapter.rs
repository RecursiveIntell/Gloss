use crate::db::notebook_db::{Chunk, NotebookDb, Source};
use crate::error::GlossError;
use crate::memory::backend::{
    excluded_source_count, filter_semantic_candidates_by_scope, invalid_requested_source_ids,
    requested_source_ids, scope_echo,
};
use crate::memory::types::{
    IndexSourceReceipt, MemorySearchRequest, MemorySearchResponse, SemanticCandidateEnvelope,
    SemanticLinkRow, MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW,
};
use crate::retrieval::source_scope::ResolvedSourceScope;
use semantic_memory::{
    ChunkManifestEntry, ChunkManifestIngestOptions, EmbeddingConfig, MemoryConfig, MemoryStore,
    ReceiptMode, SearchContext, SearchSource, SearchSourceType,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

const BACKEND_VERSION: &str = "semantic-memory 0.5.0";
const DEFAULT_SEMANTIC_MEMORY_EMBEDDING_URL: &str = "http://localhost:11434";
const DEFAULT_SEMANTIC_MEMORY_EMBEDDING_MODEL: &str = "nomic-embed-text";
const DEFAULT_SEMANTIC_MEMORY_EMBEDDING_TIMEOUT_SECS: u64 = 10;

#[derive(Debug, Clone)]
pub struct SemanticMemoryRuntimeConfig {
    pub embedding_ollama_url: String,
    pub embedding_model: String,
    pub embedding_timeout_secs: u64,
    pub turbo_quant_enabled: bool,
}

impl Default for SemanticMemoryRuntimeConfig {
    fn default() -> Self {
        Self {
            embedding_ollama_url: DEFAULT_SEMANTIC_MEMORY_EMBEDDING_URL.to_string(),
            embedding_model: DEFAULT_SEMANTIC_MEMORY_EMBEDDING_MODEL.to_string(),
            embedding_timeout_secs: DEFAULT_SEMANTIC_MEMORY_EMBEDDING_TIMEOUT_SECS,
            turbo_quant_enabled: false,
        }
    }
}

pub fn runtime_config_from_settings(
    embedding_ollama_url: Option<String>,
    embedding_model: Option<String>,
    embedding_timeout_secs: Option<String>,
    turbo_quant_enabled: bool,
) -> SemanticMemoryRuntimeConfig {
    let defaults = SemanticMemoryRuntimeConfig::default();
    SemanticMemoryRuntimeConfig {
        embedding_ollama_url: embedding_ollama_url
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(defaults.embedding_ollama_url),
        embedding_model: embedding_model
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(defaults.embedding_model),
        embedding_timeout_secs: embedding_timeout_secs
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(defaults.embedding_timeout_secs),
        turbo_quant_enabled,
    }
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
        config.embedding.ollama_url = if runtime_config.embedding_ollama_url.trim().is_empty() {
            defaults.ollama_url
        } else {
            runtime_config.embedding_ollama_url.clone()
        };
        config.embedding.model = if runtime_config.embedding_model.trim().is_empty() {
            defaults.model
        } else {
            runtime_config.embedding_model.clone()
        };
        config.embedding.timeout_secs = runtime_config.embedding_timeout_secs.max(1);
        config.limits.embedding_timeout = Duration::from_secs(config.embedding.timeout_secs);
    }
    #[cfg(feature = "semantic-memory-turbo-quant")]
    {
        if runtime_config.is_some_and(|config| config.turbo_quant_enabled) {
            config.search.derived_vector_backend =
                semantic_memory::DerivedVectorBackendPolicy::TurboQuantCandidateOnly;
            config.search.turbo_quant_require_exact_rerank = true;
        }
    }
    MemoryStore::open(config).map_err(|e| GlossError::Search(format!("semantic-memory: {e}")))
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
        nb_db.conn.execute(
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
    nb_db.conn.execute(
        "UPDATE semantic_memory_links
         SET sync_status = ?1,
             sync_error = ?2,
             synced_at = ?3
         WHERE notebook_id = ?4 AND source_id = ?5",
        rusqlite::params![status, error, now, notebook_id, source_id],
    )?;
    Ok(())
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

        nb_db.conn.execute(
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

fn chunk_manifest_entries(chunks: &[Chunk]) -> Vec<ChunkManifestEntry> {
    chunks
        .iter()
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

pub async fn reindex_source(
    data_dir: &Path,
    notebook_id: &str,
    notebook_db_path: &Path,
    source_id: &str,
    trace_id: Option<String>,
    runtime_config: Option<SemanticMemoryRuntimeConfig>,
) -> Result<IndexSourceReceipt, GlossError> {
    let (source, chunks) = {
        let nb_db = NotebookDb::connect(notebook_db_path)?;
        (
            nb_db.get_source(source_id)?,
            nb_db.get_chunks_for_source(source_id)?,
        )
    };
    if chunks.is_empty() {
        let nb_db = NotebookDb::connect(notebook_db_path)?;
        mark_source_links_status(
            &nb_db,
            notebook_id,
            source_id,
            "failed",
            Some("source has no Gloss chunks to project into semantic-memory"),
        )?;
        upsert_failed_link_rows(
            &nb_db,
            notebook_id,
            source_id,
            &chunks,
            "source has no Gloss chunks to project into semantic-memory",
        )?;
        return Ok(IndexSourceReceipt {
            backend_id: MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string(),
            notebook_id: notebook_id.to_string(),
            source_id: source_id.to_string(),
            receipt_id: trace_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            indexed_chunks: 0,
            sync_status: "failed".to_string(),
            error: Some("source has no Gloss chunks to project into semantic-memory".to_string()),
            vector_artifact_receipt: None,
        });
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

    let manifest_receipt = ingest_document_chunk_manifest(
        &store,
        ChunkManifestIngestOptions {
            title: source.title.clone(),
            namespace: notebook_id.to_string(),
            source_path: source.file_path.clone(),
            metadata: Some(metadata),
        },
        chunk_manifest_entries(&chunks),
    )
    .await?;

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
        mark_source_links_status(
            &nb_db,
            notebook_id,
            source_id,
            "stale",
            Some("source reindexed; superseded by newer semantic-memory manifest"),
        )?;
        upsert_synced_link_rows(
            &nb_db,
            notebook_id,
            source_id,
            &manifest_receipt.sm_document_id,
            &chunks,
            &mappings,
        )?;
    }
    let vector_artifact_receipt =
        rebuild_vector_artifacts_receipt(&store, runtime_config.as_ref()).await?;

    Ok(IndexSourceReceipt {
        backend_id: MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string(),
        notebook_id: notebook_id.to_string(),
        source_id: source_id.to_string(),
        receipt_id: trace_id.unwrap_or(manifest_receipt.receipt_id),
        indexed_chunks: chunks.len(),
        sync_status: "synced".to_string(),
        error: None,
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
    let mut stmt = nb_db.conn.prepare(
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
            source_scope_preserved: true,
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
    let response = store
        .search_with_context(
            &request.query,
            Some(request.limit.saturating_mul(4).max(request.limit)),
            Some(&namespaces),
            Some(&source_types),
            context,
        )
        .await
        .map_err(|e| GlossError::Search(format!("semantic-memory search failed: {e}")))?;

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
            "source_scope_preserved": true
        }),
        fallback_reason: None,
        degradation_markers,
        source_scope_preserved: true,
        fallback_used: false,
        degraded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_config_defaults_turbo_quant_off() {
        let config = runtime_config_from_settings(None, None, None, false);
        assert_eq!(
            config.embedding_ollama_url,
            DEFAULT_SEMANTIC_MEMORY_EMBEDDING_URL
        );
        assert_eq!(
            config.embedding_model,
            DEFAULT_SEMANTIC_MEMORY_EMBEDDING_MODEL
        );
        assert_eq!(
            config.embedding_timeout_secs,
            DEFAULT_SEMANTIC_MEMORY_EMBEDDING_TIMEOUT_SECS
        );
        assert!(!config.turbo_quant_enabled);
    }

    #[test]
    fn runtime_config_carries_explicit_turbo_quant_consent() {
        let config = runtime_config_from_settings(
            Some("http://localhost:11435".to_string()),
            Some("embed-model".to_string()),
            Some("12".to_string()),
            true,
        );
        assert_eq!(config.embedding_ollama_url, "http://localhost:11435");
        assert_eq!(config.embedding_model, "embed-model");
        assert_eq!(config.embedding_timeout_secs, 12);
        assert!(config.turbo_quant_enabled);
    }
}
