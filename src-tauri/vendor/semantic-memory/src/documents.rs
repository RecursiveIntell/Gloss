//! Document ingestion pipeline: chunk, embed, store, and queue sidecar updates.

use crate::chunker;
use crate::db;
#[cfg(feature = "hnsw")]
use crate::db::IndexOpKind;
use crate::error::MemoryError;
use crate::quantize::{self, Quantizer};
use crate::types::{
    ChunkManifestChunkMapping, ChunkManifestEntry, ChunkManifestIngestOptions,
    ChunkManifestIngestResult, Document, SearchResult, SearchSource,
};
use crate::{merge_trace_ctx, MemoryStore};
use rusqlite::{params, Connection};
use stack_ids::ScopeKey;
use stack_ids::TraceCtx;
use std::collections::{BTreeMap, BTreeSet};

/// A chunk plus dense/optional sparse representations and token count.
pub type ChunkRow = (
    String,
    Vec<u8>,
    Option<Vec<u8>>,
    usize,
    Option<crate::SparseWeights>,
    Option<String>,
);

pub fn insert_document_with_chunks(
    conn: &Connection,
    doc_id: &str,
    title: &str,
    namespace: &str,
    source_path: Option<&str>,
    metadata: Option<&serde_json::Value>,
    chunks: &[ChunkRow],
) -> Result<Vec<String>, MemoryError> {
    let chunk_ids: Vec<String> = (0..chunks.len())
        .map(|_| uuid::Uuid::new_v4().to_string())
        .collect();
    insert_document_with_chunks_and_ids(
        conn,
        doc_id,
        title,
        namespace,
        source_path,
        metadata,
        chunks,
        &chunk_ids,
    )?;
    Ok(chunk_ids)
}

#[allow(clippy::too_many_arguments)]
pub fn insert_document_with_chunks_and_ids(
    conn: &Connection,
    doc_id: &str,
    title: &str,
    namespace: &str,
    source_path: Option<&str>,
    metadata: Option<&serde_json::Value>,
    chunks: &[ChunkRow],
    chunk_ids: &[String],
) -> Result<(), MemoryError> {
    if chunks.len() != chunk_ids.len() {
        return Err(MemoryError::Other(
            "chunks and chunk_ids must have the same length".to_string(),
        ));
    }

    let metadata_str = metadata.map(|value| value.to_string());
    db::with_transaction(conn, |tx| {
        tx.execute(
            "INSERT INTO documents (id, title, source_path, namespace, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![doc_id, title, source_path, namespace, metadata_str],
        )?;

        for (
            chunk_index,
            (
                (content, embedding_bytes, q8_bytes, token_count, sparse, sparse_representation),
                chunk_id,
            ),
        ) in chunks.iter().zip(chunk_ids.iter()).enumerate()
        {
            tx.execute(
                "INSERT INTO chunks (id, document_id, chunk_index, content, token_count, embedding, embedding_q8)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    chunk_id,
                    doc_id,
                    chunk_index as i64,
                    content,
                    *token_count as i64,
                    embedding_bytes,
                    q8_bytes.as_deref()
                ],
            )?;

            tx.execute(
                "INSERT INTO chunks_rowid_map (chunk_id) VALUES (?1)",
                params![chunk_id],
            )?;
            let fts_rowid = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO chunks_fts (rowid, content) VALUES (?1, ?2)",
                params![fts_rowid, content],
            )?;

            #[cfg(feature = "hnsw")]
            db::queue_pending_index_op(
                tx,
                &format!("chunk:{}", chunk_id),
                "chunk",
                IndexOpKind::Upsert,
            )?;
            db::invalidate_derived_vector_artifact(tx, &format!("chunk:{chunk_id}"))?;
            if let Some((weights, representation)) =
                sparse.as_ref().zip(sparse_representation.as_deref())
            {
                db::store_sparse_vector(tx, &format!("chunk:{chunk_id}"), weights, representation)?;
            }
        }

        Ok(())
    })
}

pub fn delete_document_with_chunks(
    conn: &Connection,
    document_id: &str,
) -> Result<Vec<String>, MemoryError> {
    db::with_transaction(conn, |tx| {
        let episode_rows: Vec<(String, String, i64)> = {
            let mut stmt = tx.prepare(
                "SELECT e.episode_id, e.search_text, erm.rowid
                 FROM episodes e
                 JOIN episodes_rowid_map erm ON erm.episode_id = e.episode_id
                 WHERE e.document_id = ?1",
            )?;
            let rows = stmt.query_map(params![document_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        for (episode_id, search_text, fts_rowid) in &episode_rows {
            tx.execute(
                "INSERT INTO episodes_fts (episodes_fts, rowid, content) VALUES ('delete', ?1, ?2)",
                params![fts_rowid, search_text],
            )?;
            tx.execute(
                "DELETE FROM episodes_rowid_map WHERE episode_id = ?1",
                params![episode_id],
            )?;
            tx.execute(
                "DELETE FROM episode_causes WHERE episode_id = ?1",
                params![episode_id],
            )?;
            #[cfg(feature = "hnsw")]
            db::queue_pending_index_op(
                tx,
                &crate::episodes::episode_item_key(episode_id),
                "episode",
                IndexOpKind::Delete,
            )?;
            db::invalidate_derived_vector_artifact(
                tx,
                &crate::episodes::episode_item_key(episode_id),
            )?;
        }
        tx.execute(
            "DELETE FROM episodes WHERE document_id = ?1",
            params![document_id],
        )?;

        let mut stmt = tx.prepare(
            "SELECT c.id, c.content, cm.rowid
             FROM chunks c
             JOIN chunks_rowid_map cm ON cm.chunk_id = c.id
             WHERE c.document_id = ?1",
        )?;
        let chunk_rows: Vec<(String, String, i64)> = stmt
            .query_map(params![document_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let chunk_ids: Vec<String> = chunk_rows.iter().map(|(id, _, _)| id.clone()).collect();

        for (chunk_id, content, fts_rowid) in &chunk_rows {
            tx.execute(
                "INSERT INTO chunks_fts (chunks_fts, rowid, content) VALUES ('delete', ?1, ?2)",
                params![fts_rowid, content],
            )?;
            tx.execute(
                "DELETE FROM chunks_rowid_map WHERE chunk_id = ?1",
                params![chunk_id],
            )?;
            #[cfg(feature = "hnsw")]
            db::queue_pending_index_op(
                tx,
                &format!("chunk:{}", chunk_id),
                "chunk",
                IndexOpKind::Delete,
            )?;
            db::invalidate_derived_vector_artifact(tx, &format!("chunk:{chunk_id}"))?;
        }

        tx.execute(
            "DELETE FROM chunks WHERE document_id = ?1",
            params![document_id],
        )?;
        let affected = tx.execute("DELETE FROM documents WHERE id = ?1", params![document_id])?;
        if affected == 0 {
            return Err(MemoryError::DocumentNotFound(document_id.to_string()));
        }

        Ok(chunk_ids)
    })
}

pub fn count_chunks_for_document(
    conn: &Connection,
    document_id: &str,
) -> Result<usize, MemoryError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM chunks WHERE document_id = ?1",
        params![document_id],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}

pub fn list_documents(
    conn: &Connection,
    namespace: &str,
    limit: usize,
    offset: usize,
) -> Result<Vec<Document>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT d.id, d.title, d.source_path, d.namespace, d.created_at, d.metadata,
                (SELECT COUNT(*) FROM chunks c WHERE c.document_id = d.id) AS chunk_count
         FROM documents d
         WHERE d.namespace = ?1
         ORDER BY d.created_at DESC
         LIMIT ?2 OFFSET ?3",
    )?;

    let rows = stmt
        .query_map(params![namespace, limit as i64, offset as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)? as u32,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    rows.into_iter()
        .map(
            |(id, title, source_path, namespace, created_at, metadata_raw, chunk_count)| {
                Ok(Document {
                    metadata: db::parse_optional_json(
                        "documents",
                        &id,
                        "metadata",
                        metadata_raw.as_deref(),
                    )?,
                    id,
                    title,
                    source_path,
                    namespace,
                    created_at,
                    chunk_count,
                })
            },
        )
        .collect()
}

fn document_scope_keys_for_ids(
    conn: &Connection,
    document_ids: &[String],
) -> Result<BTreeMap<String, ScopeKey>, MemoryError> {
    if document_ids.is_empty() {
        return Ok(BTreeMap::new());
    }

    let placeholders = (0..document_ids.len())
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("SELECT id, namespace, metadata FROM documents WHERE id IN ({placeholders})");
    let params: Vec<&str> = document_ids.iter().map(|id| id.as_str()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(&params), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut by_id = BTreeMap::new();
    for (id, namespace, metadata_raw) in rows {
        let metadata =
            db::parse_optional_json("documents", &id, "metadata", metadata_raw.as_deref())?;
        let scope_key = ScopeKey {
            namespace,
            domain: metadata
                .as_ref()
                .and_then(|value| value.get("scope_domain"))
                .and_then(|value| value.as_str())
                .map(str::to_string),
            workspace_id: metadata
                .as_ref()
                .and_then(|value| value.get("scope_workspace_id"))
                .and_then(|value| value.as_str())
                .map(str::to_string),
            repo_id: metadata
                .as_ref()
                .and_then(|value| value.get("scope_repo_id"))
                .and_then(|value| value.as_str())
                .map(str::to_string),
        };
        by_id.insert(id, scope_key);
    }

    Ok(by_id)
}

impl MemoryStore {
    /// Ingest a document: chunk, embed all chunks, store everything.
    pub async fn ingest_document(
        &self,
        title: &str,
        content: &str,
        namespace: &str,
        source_path: Option<&str>,
        metadata: Option<serde_json::Value>,
    ) -> Result<String, MemoryError> {
        self.ingest_document_with_trace(title, content, namespace, source_path, metadata, None)
            .await
    }

    /// Ingest a document with optional trace metadata.
    pub async fn ingest_document_with_trace(
        &self,
        title: &str,
        content: &str,
        namespace: &str,
        source_path: Option<&str>,
        metadata: Option<serde_json::Value>,
        trace_ctx: Option<&TraceCtx>,
    ) -> Result<String, MemoryError> {
        self.validate_content("document.content", content)?;

        // Dedup: check if a document with the same title already exists
        // in the same namespace. Return the existing document ID instead
        // of creating a duplicate.
        let title_check = title.to_string();
        let ns_check = namespace.to_string();
        let existing_id = self
            .with_read_conn(move |conn| {
                let result: Option<String> = conn
                    .query_row(
                        "SELECT id FROM documents WHERE title = ?1 AND namespace = ?2 LIMIT 1",
                        rusqlite::params![&title_check, &ns_check],
                        |row| row.get::<_, String>(0),
                    )
                    .ok();
                Ok(result)
            })
            .await?;

        if let Some(id) = existing_id {
            return Ok(id);
        }

        let text_chunks = chunker::chunk_text(
            content,
            &self.inner.config.chunking,
            self.inner.token_counter.as_ref(),
        );

        let max_chunks = self.inner.config.limits.max_chunks_per_document;
        if text_chunks.len() > max_chunks {
            return Err(MemoryError::ContentTooLarge {
                size: text_chunks.len(),
                limit: max_chunks,
            });
        }

        let chunk_texts: Vec<String> = text_chunks.iter().map(|c| c.content.clone()).collect();
        let embeddings = self
            .embed_batch_with_sparse_internal(chunk_texts, crate::EmbeddingPurpose::Document)
            .await?;

        let quantizer = Quantizer::new(self.inner.config.embedding.dimensions);
        let chunks: Vec<ChunkRow> = text_chunks
            .iter()
            .zip(embeddings.iter())
            .map(|(tc, (emb, sparse, sparse_representation))| {
                // INTENTIONAL: q8 quantization is an optional search optimization; missing q8 is non-fatal
                let q8 = quantizer
                    .quantize(emb)
                    .map(|qv| quantize::pack_quantized(&qv))
                    .ok();
                (
                    tc.content.clone(),
                    db::embedding_to_bytes(emb),
                    q8,
                    tc.token_count_estimate,
                    sparse.clone(),
                    sparse_representation.clone(),
                )
            })
            .collect();

        let doc_id = uuid::Uuid::new_v4().to_string();

        let did = doc_id.clone();
        let t = title.to_string();
        let ns = namespace.to_string();
        let sp = source_path.map(|s| s.to_string());
        let meta = merge_trace_ctx(metadata, trace_ctx);

        self.with_write_conn(move |conn| {
            insert_document_with_chunks(conn, &did, &t, &ns, sp.as_deref(), meta.as_ref(), &chunks)
        })
        .await?;

        #[cfg(feature = "hnsw")]
        self.sync_pending_hnsw_ops_best_effort("ingest_document")
            .await;

        Ok(doc_id)
    }

    /// Ingest an externally chunked document manifest and return exact chunk mappings.
    ///
    /// This API preserves semantic-memory as the owner of document/chunk storage and embeddings:
    /// callers provide chunk boundaries and external IDs, while semantic-memory generates and
    /// stores its own document/chunk IDs atomically.
    pub async fn ingest_chunk_manifest(
        &self,
        options: ChunkManifestIngestOptions,
        entries: Vec<ChunkManifestEntry>,
    ) -> Result<ChunkManifestIngestResult, MemoryError> {
        self.ingest_chunk_manifest_with_trace(options, entries, None)
            .await
    }

    /// Ingest an externally chunked document manifest with optional trace metadata.
    pub async fn ingest_chunk_manifest_with_trace(
        &self,
        options: ChunkManifestIngestOptions,
        entries: Vec<ChunkManifestEntry>,
        trace_ctx: Option<&TraceCtx>,
    ) -> Result<ChunkManifestIngestResult, MemoryError> {
        if entries.is_empty() {
            return Err(MemoryError::InvalidConfig {
                field: "chunk_manifest.entries",
                reason: "at least one chunk is required".to_string(),
            });
        }

        let max_chunks = self.inner.config.limits.max_chunks_per_document;
        if entries.len() > max_chunks {
            return Err(MemoryError::ContentTooLarge {
                size: entries.len(),
                limit: max_chunks,
            });
        }

        let mut seen_external_ids = BTreeSet::new();
        for (index, entry) in entries.iter().enumerate() {
            let external_chunk_id = entry.external_chunk_id.trim();
            if external_chunk_id.is_empty() {
                return Err(MemoryError::InvalidConfig {
                    field: "chunk_manifest.external_chunk_id",
                    reason: format!("chunk {index} external_chunk_id must not be empty"),
                });
            }
            if !seen_external_ids.insert(external_chunk_id.to_string()) {
                return Err(MemoryError::InvalidConfig {
                    field: "chunk_manifest.external_chunk_id",
                    reason: format!("duplicate external_chunk_id '{external_chunk_id}'"),
                });
            }
            if entry.content.trim().is_empty() {
                return Err(MemoryError::InvalidConfig {
                    field: "chunk_manifest.content",
                    reason: format!(
                        "content must not be empty (chunk index {index}, id='{external_chunk_id}')"
                    ),
                });
            }
            self.validate_content("chunk_manifest.content", &entry.content)?;
            if entry
                .content_digest
                .as_deref()
                .is_some_and(|digest| digest.trim().is_empty())
            {
                return Err(MemoryError::InvalidConfig {
                    field: "chunk_manifest.content_digest",
                    reason: format!("chunk {index} content_digest must not be empty when supplied"),
                });
            }
        }

        let chunk_texts: Vec<String> = entries.iter().map(|entry| entry.content.clone()).collect();
        let embeddings = self
            .embed_batch_with_sparse_internal(chunk_texts, crate::EmbeddingPurpose::Document)
            .await?;

        let quantizer = Quantizer::new(self.inner.config.embedding.dimensions);
        let chunks: Vec<ChunkRow> = entries
            .iter()
            .zip(embeddings.iter())
            .map(|(entry, (emb, sparse, sparse_representation))| {
                let q8 = quantizer
                    .quantize(emb)
                    .map(|qv| quantize::pack_quantized(&qv))
                    .ok();
                (
                    entry.content.clone(),
                    db::embedding_to_bytes(emb),
                    q8,
                    entry
                        .token_count_estimate
                        .unwrap_or_else(|| entry.content.len().div_ceil(4).max(1)),
                    sparse.clone(),
                    sparse_representation.clone(),
                )
            })
            .collect();

        let doc_id = uuid::Uuid::new_v4().to_string();
        let chunk_ids: Vec<String> = (0..entries.len())
            .map(|_| uuid::Uuid::new_v4().to_string())
            .collect();
        let receipt_id = format!("chunk-manifest:{}", uuid::Uuid::new_v4());

        let mappings: Vec<ChunkManifestChunkMapping> = entries
            .iter()
            .zip(chunk_ids.iter())
            .enumerate()
            .map(
                |(chunk_index, (entry, sm_chunk_id))| ChunkManifestChunkMapping {
                    external_chunk_id: entry.external_chunk_id.clone(),
                    sm_document_id: doc_id.clone(),
                    sm_chunk_id: sm_chunk_id.clone(),
                    chunk_index,
                    content_digest: entry.content_digest.clone(),
                    metadata: entry.metadata.clone(),
                },
            )
            .collect();

        let did = doc_id.clone();
        let title = options.title;
        let namespace = options.namespace;
        let source_path = options.source_path;
        let metadata = merge_trace_ctx(options.metadata, trace_ctx);
        let namespace_for_result = namespace.clone();

        self.with_write_conn(move |conn| {
            insert_document_with_chunks_and_ids(
                conn,
                &did,
                &title,
                &namespace,
                source_path.as_deref(),
                metadata.as_ref(),
                &chunks,
                &chunk_ids,
            )
        })
        .await?;

        #[cfg(feature = "hnsw")]
        self.sync_pending_hnsw_ops_best_effort("ingest_chunk_manifest")
            .await;

        Ok(ChunkManifestIngestResult {
            sm_document_id: doc_id,
            namespace: namespace_for_result,
            receipt_id,
            chunks: mappings,
        })
    }

    /// Delete a document and all its chunks.
    pub async fn delete_document(&self, document_id: &str) -> Result<(), MemoryError> {
        let did = document_id.to_string();
        self.with_write_conn(move |conn| delete_document_with_chunks(conn, &did))
            .await?;

        #[cfg(feature = "hnsw")]
        self.sync_pending_hnsw_ops_best_effort("delete_document")
            .await;

        Ok(())
    }

    /// List documents in a namespace.
    pub async fn list_documents(
        &self,
        namespace: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Document>, MemoryError> {
        let ns = namespace.to_string();
        self.with_read_conn(move |conn| list_documents(conn, &ns, limit, offset))
            .await
    }

    /// Count the number of chunks for a document.
    pub async fn count_chunks_for_document(&self, document_id: &str) -> Result<usize, MemoryError> {
        let did = document_id.to_string();
        self.with_read_conn(move |conn| count_chunks_for_document(conn, &did))
            .await
    }

    /// Filter search results to those whose source scope exactly matches the requested scope.
    ///
    /// Only source families that carry or can be joined to full scope metadata are retained:
    /// chunks, episodes, and imported projection rows. Facts and messages are excluded because
    /// they do not carry domain/workspace/repo provenance.
    pub async fn filter_search_results_by_scope(
        &self,
        results: Vec<SearchResult>,
        scope: &ScopeKey,
    ) -> Result<Vec<SearchResult>, MemoryError> {
        let mut document_ids = BTreeSet::new();
        for result in &results {
            match &result.source {
                SearchSource::Chunk { document_id, .. }
                | SearchSource::Episode { document_id, .. } => {
                    document_ids.insert(document_id.clone());
                }
                _ => {}
            }
        }

        let document_ids = document_ids.into_iter().collect::<Vec<_>>();
        let scope_by_document = self
            .with_read_conn(move |conn| document_scope_keys_for_ids(conn, &document_ids))
            .await?;
        let requested = scope.clone();

        Ok(results
            .into_iter()
            .filter(|result| match &result.source {
                SearchSource::Chunk { document_id, .. }
                | SearchSource::Episode { document_id, .. } => scope_by_document
                    .get(document_id)
                    .map(|scope_key| scope_key == &requested)
                    .unwrap_or(false),
                SearchSource::Projection { scope_key, .. } => scope_key == &requested,
                SearchSource::Fact { .. } | SearchSource::Message { .. } => false,
            })
            .collect())
    }
}
