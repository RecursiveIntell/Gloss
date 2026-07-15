use crate::db;
#[cfg(feature = "hnsw")]
use crate::db::IndexOpKind;
use crate::error::MemoryError;
use crate::quantize::{self, Quantizer};
use crate::types::{EpisodeMeta, EpisodeOutcome, VerificationStatus};
use crate::{build_episode_search_text, verification_status_for_outcome, MemoryStore};
use rusqlite::{params, Connection};
use stack_ids::{DigestBuilder, TraceCtx};
use std::collections::BTreeSet;

// ─── Centralized episode identity helpers ──────────────────────────────

/// Canonical HNSW item key for an episode.
pub(crate) fn episode_item_key(episode_id: &str) -> String {
    format!("episode:{episode_id}")
}

/// Canonical graph node ID for an episode.
pub(crate) fn episode_node_id(episode_id: &str) -> String {
    format!("episode:{episode_id}")
}

/// Resolve the primary (first-created) episode_id for a document.
/// This is **legacy compatibility** behavior for APIs that still target
/// a single episode by document_id. Canonical code should use episode_id directly.
pub(crate) fn resolve_primary_episode_id_legacy(
    conn: &Connection,
    document_id: &str,
) -> Result<Option<String>, MemoryError> {
    match conn.query_row(
        "SELECT episode_id FROM episodes WHERE document_id = ?1 ORDER BY created_at ASC LIMIT 1",
        params![document_id],
        |row| row.get(0),
    ) {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(MemoryError::Database(err)),
    }
}

/// List all episode_ids for a document, ordered by creation time.
pub(crate) fn list_document_episode_ids(
    conn: &Connection,
    document_id: &str,
) -> Result<Vec<String>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT episode_id FROM episodes WHERE document_id = ?1 ORDER BY created_at ASC",
    )?;
    let ids = stmt
        .query_map(params![document_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// Insert a new episode with an explicit episode_id (canonical path).
/// Returns the episode_id.
#[allow(clippy::too_many_arguments)]
pub(crate) fn create_episode(
    conn: &Connection,
    episode_id: &str,
    document_id: &str,
    meta: &EpisodeMeta,
    search_text: &str,
    embedding_bytes: &[u8],
    q8_bytes: Option<&[u8]>,
    trace_id: Option<&str>,
) -> Result<String, MemoryError> {
    let cause_ids_json =
        serde_json::to_string(&meta.cause_ids).map_err(|e| MemoryError::Other(e.to_string()))?;
    let verification_json = serde_json::to_string(&meta.verification_status)
        .map_err(|e| MemoryError::Other(e.to_string()))?;
    let item_key = episode_item_key(episode_id);

    db::with_transaction(conn, |tx| {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM documents WHERE id = ?1)",
            params![document_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(MemoryError::DocumentNotFound(document_id.to_string()));
        }

        tx.execute(
            "INSERT INTO episodes
                (episode_id, document_id, cause_ids, effect_type, outcome, confidence,
                 verification_status, experiment_id, search_text, embedding, embedding_q8,
                 trace_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, datetime('now'))",
            params![
                episode_id,
                document_id,
                cause_ids_json,
                meta.effect_type,
                meta.outcome.as_str(),
                meta.confidence,
                verification_json,
                meta.experiment_id,
                search_text,
                embedding_bytes,
                q8_bytes,
                trace_id
            ],
        )?;

        // Insert FTS mapping
        tx.execute(
            "INSERT INTO episodes_rowid_map (episode_id, document_id) VALUES (?1, ?2)",
            params![episode_id, document_id],
        )?;
        let fts_rowid: i64 = tx.query_row(
            "SELECT rowid FROM episodes_rowid_map WHERE episode_id = ?1",
            params![episode_id],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO episodes_fts (rowid, content) VALUES (?1, ?2)",
            params![fts_rowid, search_text],
        )?;

        // Populate normalized causal edges
        sync_causal_edges(tx, episode_id, &meta.cause_ids)?;

        #[cfg(feature = "hnsw")]
        db::queue_pending_index_op(tx, &item_key, "episode", IndexOpKind::Upsert)?;
        db::invalidate_derived_vector_artifact(tx, &item_key)?;
        Ok(episode_id.to_string())
    })
}

/// Legacy compatibility: upsert the primary episode for a document.
///
/// If an episode already exists for this document, updates the first one.
/// Otherwise creates a new one with a deterministic `{document_id}-ep0` episode_id.
/// Canonical callers should use `create_episode()` with an explicit episode_id instead.
#[allow(clippy::too_many_arguments)]
pub(crate) fn upsert_episode(
    conn: &Connection,
    document_id: &str,
    meta: &EpisodeMeta,
    search_text: &str,
    embedding_bytes: &[u8],
    q8_bytes: Option<&[u8]>,
    trace_id: Option<&str>,
) -> Result<String, MemoryError> {
    let cause_ids_json =
        serde_json::to_string(&meta.cause_ids).map_err(|e| MemoryError::Other(e.to_string()))?;
    let verification_json = serde_json::to_string(&meta.verification_status)
        .map_err(|e| MemoryError::Other(e.to_string()))?;

    // Legacy compat: resolve the primary episode for this document
    let existing_episode_id = resolve_primary_episode_id_legacy(conn, document_id)?;

    let episode_id = existing_episode_id.unwrap_or_else(|| format!("{}-ep0", document_id));

    let item_key = episode_item_key(&episode_id);

    db::with_transaction(conn, |tx| {
        // INTENTIONAL: episode may not exist yet on first upsert
        let old_search_text: Option<String> = tx
            .query_row(
                "SELECT search_text FROM episodes WHERE episode_id = ?1",
                params![episode_id],
                |row| row.get(0),
            )
            .ok();
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM documents WHERE id = ?1)",
            params![document_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(MemoryError::DocumentNotFound(document_id.to_string()));
        }

        if old_search_text.is_some() {
            // ── Bitemporal append-supersede via UPDATE ──────────────────────────────
            // Read the current row's fact_digest (if any) so we can mark what it supersedes.
            let prior_fact_digest: Option<String> = tx
                .query_row(
                    "SELECT fact_digest FROM episodes WHERE episode_id = ?1
                     ORDER BY recorded_time DESC LIMIT 1",
                    params![episode_id],
                    |row| row.get(0),
                )
                .ok()
                .flatten();

            // Compute content-addressed digest of the new fact payload.
            let mut digest_builder = DigestBuilder::new();
            digest_builder.update_str("semantic-memory.episode.v1");
            digest_builder.separator();
            digest_builder.update_str(&cause_ids_json);
            digest_builder.separator();
            digest_builder.update_str(meta.effect_type.as_str());
            digest_builder.separator();
            digest_builder.update_str(meta.outcome.as_str());
            digest_builder.separator();
            digest_builder.update(&meta.confidence.to_le_bytes());
            let new_fact_digest = format!("blake3:{}", digest_builder.finalize().hex());

            // valid_time: when this episode fact is true in the domain
            let valid_time_sql: Option<String> =
                meta.valid_time.map(|dt| format!("'{}'", dt.to_rfc3339()));

            // Advance the row in place: new recorded_time, new valid_time,
            // superseded_by chains to prior_fact_digest, fact_digest is the new digest.
            tx.execute(
                &format!(
                    "UPDATE episodes SET
                         cause_ids = ?1,
                         effect_type = ?2,
                         outcome = ?3,
                         confidence = ?4,
                         verification_status = ?5,
                         experiment_id = ?6,
                         search_text = ?7,
                         embedding = ?8,
                         embedding_q8 = ?9,
                         trace_id = COALESCE(?10, trace_id),
                         updated_at = datetime('now'),
                         valid_time = {},
                         recorded_time = datetime('now'),
                         superseded_by = ?11,
                         fact_digest = ?12
                     WHERE episode_id = ?13",
                    valid_time_sql.as_deref().unwrap_or("NULL"),
                ),
                params![
                    cause_ids_json,
                    meta.effect_type,
                    meta.outcome.as_str(),
                    meta.confidence,
                    verification_json,
                    meta.experiment_id,
                    search_text,
                    embedding_bytes,
                    q8_bytes,
                    trace_id,
                    prior_fact_digest,
                    new_fact_digest,
                    episode_id,
                ],
            )?;
            // FTS entry already exists for this episode; search_text update is a no-op for FTS
            // since the existing rowid stays the same — the content change is handled by
            // the text-based search query, not a separate FTS entry.
        } else {
            // Insert new episode
            tx.execute(
                "INSERT INTO episodes
                    (episode_id, document_id, cause_ids, effect_type, outcome, confidence,
                     verification_status, experiment_id, search_text, embedding, embedding_q8,
                     trace_id, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, datetime('now'))",
                params![
                    episode_id,
                    document_id,
                    cause_ids_json,
                    meta.effect_type,
                    meta.outcome.as_str(),
                    meta.confidence,
                    verification_json,
                    meta.experiment_id,
                    search_text,
                    embedding_bytes,
                    q8_bytes,
                    trace_id
                ],
            )?;

            // Insert FTS mapping
            tx.execute(
                "INSERT INTO episodes_rowid_map (episode_id, document_id) VALUES (?1, ?2)",
                params![episode_id, document_id],
            )?;
            let fts_rowid: i64 = tx.query_row(
                "SELECT rowid FROM episodes_rowid_map WHERE episode_id = ?1",
                params![episode_id],
                |row| row.get(0),
            )?;
            tx.execute(
                "INSERT INTO episodes_fts (rowid, content) VALUES (?1, ?2)",
                params![fts_rowid, search_text],
            )?;
        }

        // Sync normalized causal edges
        sync_causal_edges(tx, &episode_id, &meta.cause_ids)?;

        #[cfg(feature = "hnsw")]
        db::queue_pending_index_op(tx, &item_key, "episode", IndexOpKind::Upsert)?;
        db::invalidate_derived_vector_artifact(tx, &item_key)?;
        Ok(episode_id.to_string())
    })
}

/// Synchronize the episode_causes table with the given cause_ids.
fn sync_causal_edges(
    tx: &rusqlite::Transaction<'_>,
    episode_id: &str,
    cause_ids: &[String],
) -> Result<(), MemoryError> {
    let mut seen = BTreeSet::new();
    for cause_id in cause_ids {
        if !seen.insert(cause_id) {
            return Err(MemoryError::InvalidConfig {
                field: "episodes.cause_ids",
                reason: format!("duplicate cause id: {cause_id}"),
            });
        }
    }
    tx.execute(
        "DELETE FROM episode_causes WHERE episode_id = ?1",
        params![episode_id],
    )?;
    for (ordinal, cause_id) in cause_ids.iter().enumerate() {
        tx.execute(
            "INSERT INTO episode_causes (episode_id, cause_node_id, ordinal)
             VALUES (?1, ?2, ?3)",
            params![episode_id, cause_id, ordinal as i64],
        )?;
    }
    Ok(())
}

/// Legacy compatibility: update the primary episode's outcome for a document.
/// Resolves the first-created episode and delegates to `update_episode_outcome_by_id`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_episode_outcome(
    conn: &Connection,
    document_id: &str,
    outcome: EpisodeOutcome,
    confidence: f32,
    experiment_id: Option<&str>,
    verification_status: &VerificationStatus,
    search_text: &str,
    embedding_bytes: &[u8],
    q8_bytes: Option<&[u8]>,
) -> Result<(), MemoryError> {
    // Legacy compat: resolve the primary episode for this document
    let episode_id = resolve_primary_episode_id_legacy(conn, document_id)?
        .ok_or_else(|| MemoryError::DocumentNotFound(document_id.to_string()))?;

    update_episode_outcome_by_id(
        conn,
        &episode_id,
        outcome,
        confidence,
        experiment_id,
        verification_status,
        search_text,
        embedding_bytes,
        q8_bytes,
    )
}

/// Update the outcome of an episode by its episode_id (canonical path).
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_episode_outcome_by_id(
    conn: &Connection,
    episode_id: &str,
    outcome: EpisodeOutcome,
    confidence: f32,
    experiment_id: Option<&str>,
    verification_status: &VerificationStatus,
    search_text: &str,
    embedding_bytes: &[u8],
    q8_bytes: Option<&[u8]>,
) -> Result<(), MemoryError> {
    let verification_json = serde_json::to_string(verification_status)
        .map_err(|e| MemoryError::Other(e.to_string()))?;
    let item_key = episode_item_key(episode_id);

    db::with_transaction(conn, |tx| {
        let old_search_text: String = tx
            .query_row(
                "SELECT search_text FROM episodes WHERE episode_id = ?1",
                params![episode_id],
                |row| row.get(0),
            )
            .map_err(|e| MemoryError::EpisodeNotFound(format!("{}: {e}", episode_id)))?;
        let fts_rowid: i64 = tx.query_row(
            "SELECT rowid FROM episodes_rowid_map WHERE episode_id = ?1",
            params![episode_id],
            |row| row.get(0),
        )?;

        tx.execute(
            "INSERT INTO episodes_fts (episodes_fts, rowid, content) VALUES ('delete', ?1, ?2)",
            params![fts_rowid, old_search_text],
        )?;
        tx.execute(
            "UPDATE episodes
             SET outcome = ?1,
                 confidence = ?2,
                 experiment_id = COALESCE(?3, experiment_id),
                 verification_status = ?4,
                 search_text = ?5,
                 embedding = ?6,
                 embedding_q8 = ?7,
                 updated_at = datetime('now')
             WHERE episode_id = ?8",
            params![
                outcome.as_str(),
                confidence,
                experiment_id,
                verification_json,
                search_text,
                embedding_bytes,
                q8_bytes,
                episode_id
            ],
        )?;
        tx.execute(
            "INSERT INTO episodes_fts (rowid, content) VALUES (?1, ?2)",
            params![fts_rowid, search_text],
        )?;

        #[cfg(feature = "hnsw")]
        db::queue_pending_index_op(tx, &item_key, "episode", IndexOpKind::Upsert)?;
        db::invalidate_derived_vector_artifact(tx, &item_key)?;
        Ok(())
    })
}

pub(crate) fn search_episodes(
    conn: &Connection,
    effect_type: Option<&str>,
    outcome: Option<&EpisodeOutcome>,
    limit: usize,
) -> Result<Vec<(String, EpisodeMeta)>, MemoryError> {
    const MAX_EPISODE_SEARCH_LIMIT: usize = 1_000;
    let limit = limit.clamp(1, MAX_EPISODE_SEARCH_LIMIT);
    let effect_type = effect_type.map(ToOwned::to_owned);
    let outcome = outcome.map(|value| value.as_str().to_string());

    let mut sql = String::from(
        "SELECT episode_id, document_id, cause_ids, effect_type, outcome, confidence, verification_status, experiment_id
         FROM episodes
         WHERE 1 = 1",
    );
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(effect_type) = &effect_type {
        sql.push_str(&format!(" AND effect_type = ?{}", params.len() + 1));
        params.push(Box::new(effect_type.clone()));
    }
    if let Some(outcome) = &outcome {
        sql.push_str(&format!(" AND outcome = ?{}", params.len() + 1));
        params.push(Box::new(outcome.clone()));
    }
    let limit_param = params.len() + 1;
    sql.push_str(&format!(" ORDER BY updated_at DESC LIMIT ?{}", limit_param));
    params.push(Box::new(limit as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        params.iter().map(|value| value.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(&*param_refs, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, f32>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    rows.into_iter()
        .map(
            |(
                episode_id,
                _document_id,
                cause_ids_raw,
                effect_type,
                outcome_raw,
                confidence,
                verification_status_raw,
                experiment_id,
            )| {
                Ok((
                    episode_id.clone(),
                    EpisodeMeta {
                        cause_ids: db::parse_string_list_json(
                            "episodes",
                            &episode_id,
                            "cause_ids",
                            &cause_ids_raw,
                        )?,
                        effect_type,
                        outcome: db::parse_episode_outcome(&episode_id, &outcome_raw)?,
                        confidence,
                        verification_status: db::parse_verification_status(
                            &episode_id,
                            &verification_status_raw,
                        )?,
                        experiment_id,
                        valid_time: None,
                        fact_digest: None,
                    },
                ))
            },
        )
        .collect()
}

/// Get episode by episode_id.
pub(crate) fn get_episode(
    conn: &Connection,
    episode_id: &str,
) -> Result<Option<(String, EpisodeMeta)>, MemoryError> {
    let row = conn.query_row(
        "SELECT document_id, cause_ids, effect_type, outcome, confidence, verification_status, experiment_id
         FROM episodes
         WHERE episode_id = ?1",
        params![episode_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, f32>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        },
    );

    match row {
        Ok((
            document_id,
            cause_ids_raw,
            effect_type,
            outcome_raw,
            confidence,
            verification_status_raw,
            experiment_id,
        )) => Ok(Some((
            document_id.clone(),
            EpisodeMeta {
                cause_ids: db::parse_string_list_json(
                    "episodes",
                    episode_id,
                    "cause_ids",
                    &cause_ids_raw,
                )?,
                effect_type,
                outcome: db::parse_episode_outcome(episode_id, &outcome_raw)?,
                confidence,
                verification_status: db::parse_verification_status(
                    episode_id,
                    &verification_status_raw,
                )?,
                experiment_id,
                valid_time: None,
                fact_digest: None,
            },
        ))),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(MemoryError::Database(err)),
    }
}

/// Legacy compatibility: load the primary episode's metadata for a document.
/// Returns the first-created episode's metadata, or None if no episodes exist.
pub(crate) fn load_episode_meta(
    conn: &Connection,
    document_id: &str,
) -> Result<Option<EpisodeMeta>, MemoryError> {
    let row = conn.query_row(
        "SELECT cause_ids, effect_type, outcome, confidence, verification_status, experiment_id
         FROM episodes
         WHERE document_id = ?1
         ORDER BY created_at ASC
         LIMIT 1",
        params![document_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f32>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        },
    );

    match row {
        Ok((
            cause_ids_raw,
            effect_type,
            outcome_raw,
            confidence,
            verification_status_raw,
            experiment_id,
        )) => Ok(Some(EpisodeMeta {
            cause_ids: db::parse_string_list_json(
                "episodes",
                document_id,
                "cause_ids",
                &cause_ids_raw,
            )?,
            effect_type,
            outcome: db::parse_episode_outcome(document_id, &outcome_raw)?,
            confidence,
            verification_status: db::parse_verification_status(
                document_id,
                &verification_status_raw,
            )?,
            experiment_id,
            valid_time: None,
            fact_digest: None,
        })),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(MemoryError::Database(err)),
    }
}

pub(crate) fn load_episode_context(
    conn: &Connection,
    document_id: &str,
) -> Result<(String, String), MemoryError> {
    let title: String = conn
        .query_row(
            "SELECT title FROM documents WHERE id = ?1",
            params![document_id],
            |row| row.get(0),
        )
        .map_err(|e| MemoryError::DocumentNotFound(format!("{}: {e}", document_id)))?;

    let mut stmt =
        conn.prepare("SELECT content FROM chunks WHERE document_id = ?1 ORDER BY chunk_index ASC")?;
    let chunks = stmt
        .query_map(params![document_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok((title, chunks.join("\n")))
}

impl MemoryStore {
    /// Ingest or update a causal episode attached to a document.
    ///
    /// The document must already exist. Existing episodes keep their original `created_at`
    /// timestamp while their searchable text, outcome state, verification metadata, embeddings,
    /// and `updated_at` are refreshed.
    pub async fn ingest_episode(
        &self,
        document_id: &str,
        meta: &EpisodeMeta,
    ) -> Result<String, MemoryError> {
        self.ingest_episode_with_trace(document_id, meta, None)
            .await
    }

    /// Ingest a causal episode with optional trace metadata. Returns the episode_id.
    pub async fn ingest_episode_with_trace(
        &self,
        document_id: &str,
        meta: &EpisodeMeta,
        trace_ctx: Option<&TraceCtx>,
    ) -> Result<String, MemoryError> {
        self.validate_content("episodes.effect_type", &meta.effect_type)?;
        Self::validate_confidence(meta.confidence)?;
        let doc_id = document_id.to_string();
        let meta = meta.clone();
        let (document_title, document_context) = self
            .with_read_conn(move |conn| load_episode_context(conn, &doc_id))
            .await?;
        let search_text = build_episode_search_text(&document_title, &document_context, &meta);
        let (embedding, sparse, sparse_representation) = self
            .embed_text_with_sparse_internal(&search_text, crate::EmbeddingPurpose::Document)
            .await?;
        self.validate_embedding_dimensions(&embedding)?;
        let embedding_bytes = db::embedding_to_bytes(&embedding);
        // INTENTIONAL: q8 quantization is an optional search optimization; missing q8 is non-fatal
        let q8_bytes = Quantizer::new(self.inner.config.embedding.dimensions)
            .quantize(&embedding)
            .map(|vector| quantize::pack_quantized(&vector))
            .ok();
        let trace_id_owned = trace_ctx.map(|value| value.trace_id.clone());

        let doc_id = document_id.to_string();
        let episode_id = self
            .with_write_conn(move |conn| {
                let episode_id = upsert_episode(
                    conn,
                    &doc_id,
                    &meta,
                    &search_text,
                    &embedding_bytes,
                    q8_bytes.as_deref(),
                    trace_id_owned.as_deref(),
                )?;
                if let Some((weights, representation)) =
                    sparse.as_ref().zip(sparse_representation.as_deref())
                {
                    db::store_sparse_vector(
                        conn,
                        &episode_item_key(&episode_id),
                        weights,
                        representation,
                    )?;
                }
                Ok(episode_id)
            })
            .await?;

        #[cfg(feature = "hnsw")]
        self.sync_pending_hnsw_ops_best_effort("ingest_episode")
            .await;

        Ok(episode_id)
    }

    /// Create a new episode with an explicit episode_id. Returns the episode_id.
    pub async fn create_episode(
        &self,
        episode_id: &str,
        document_id: &str,
        meta: &EpisodeMeta,
    ) -> Result<String, MemoryError> {
        self.create_episode_with_trace(episode_id, document_id, meta, None)
            .await
    }

    /// Create a new episode with an explicit episode_id and optional trace metadata.
    pub async fn create_episode_with_trace(
        &self,
        episode_id: &str,
        document_id: &str,
        meta: &EpisodeMeta,
        trace_ctx: Option<&TraceCtx>,
    ) -> Result<String, MemoryError> {
        self.validate_content("episodes.effect_type", &meta.effect_type)?;
        Self::validate_confidence(meta.confidence)?;
        let doc_id = document_id.to_string();
        let meta = meta.clone();
        let (document_title, document_context) = self
            .with_read_conn(move |conn| load_episode_context(conn, &doc_id))
            .await?;
        let search_text = build_episode_search_text(&document_title, &document_context, &meta);
        let (embedding, sparse, sparse_representation) = self
            .embed_text_with_sparse_internal(&search_text, crate::EmbeddingPurpose::Document)
            .await?;
        self.validate_embedding_dimensions(&embedding)?;
        let embedding_bytes = db::embedding_to_bytes(&embedding);
        // INTENTIONAL: q8 quantization is an optional search optimization; missing q8 is non-fatal
        let q8_bytes = Quantizer::new(self.inner.config.embedding.dimensions)
            .quantize(&embedding)
            .map(|vector| quantize::pack_quantized(&vector))
            .ok();
        let trace_id_owned = trace_ctx.map(|value| value.trace_id.clone());

        let ep_id = episode_id.to_string();
        let doc_id = document_id.to_string();
        let created_ep_id = self
            .with_write_conn(move |conn| {
                let created_id = crate::episodes::create_episode(
                    conn,
                    &ep_id,
                    &doc_id,
                    &meta,
                    &search_text,
                    &embedding_bytes,
                    q8_bytes.as_deref(),
                    trace_id_owned.as_deref(),
                )?;
                if let Some((weights, representation)) =
                    sparse.as_ref().zip(sparse_representation.as_deref())
                {
                    db::store_sparse_vector(
                        conn,
                        &episode_item_key(&created_id),
                        weights,
                        representation,
                    )?;
                }
                Ok(created_id)
            })
            .await?;

        #[cfg(feature = "hnsw")]
        self.sync_pending_hnsw_ops_best_effort("create_episode")
            .await;

        Ok(created_ep_id)
    }

    /// Retrieve an episode by its episode_id.
    pub async fn get_episode(
        &self,
        episode_id: &str,
    ) -> Result<Option<(String, EpisodeMeta)>, MemoryError> {
        let ep_id = episode_id.to_string();
        self.with_read_conn(move |conn| get_episode(conn, &ep_id))
            .await
    }

    /// Update the outcome of an episode by its episode_id.
    pub async fn update_episode_outcome_by_id(
        &self,
        episode_id: &str,
        outcome: EpisodeOutcome,
        confidence: f32,
        experiment_id: Option<&str>,
    ) -> Result<(), MemoryError> {
        Self::validate_confidence(confidence)?;
        let ep_id = episode_id.to_string();
        let ep_id_clone = ep_id.clone();
        let (doc_id, current_meta) = self
            .with_read_conn(move |conn| {
                get_episode(conn, &ep_id_clone)?
                    .ok_or_else(|| MemoryError::EpisodeNotFound(ep_id_clone.clone()))
            })
            .await?;

        let experiment_id_owned = experiment_id.map(|value| value.to_string());
        let verification_status =
            verification_status_for_outcome(&outcome, experiment_id_owned.as_deref());
        let updated_meta = EpisodeMeta {
            cause_ids: current_meta.cause_ids,
            effect_type: current_meta.effect_type,
            outcome: outcome.clone(),
            confidence,
            verification_status: verification_status.clone(),
            experiment_id: experiment_id_owned.clone().or(current_meta.experiment_id),
            valid_time: current_meta.valid_time,
            fact_digest: current_meta.fact_digest.clone(),
        };

        let (document_title, document_context) = self
            .with_read_conn(move |conn| load_episode_context(conn, &doc_id))
            .await?;
        let search_text =
            build_episode_search_text(&document_title, &document_context, &updated_meta);
        let (embedding, sparse, sparse_representation) = self
            .embed_text_with_sparse_internal(&search_text, crate::EmbeddingPurpose::Document)
            .await?;
        self.validate_embedding_dimensions(&embedding)?;
        let embedding_bytes = db::embedding_to_bytes(&embedding);
        // INTENTIONAL: q8 quantization is an optional search optimization; missing q8 is non-fatal
        let q8_bytes = Quantizer::new(self.inner.config.embedding.dimensions)
            .quantize(&embedding)
            .map(|vector| quantize::pack_quantized(&vector))
            .ok();

        self.with_write_conn(move |conn| {
            crate::episodes::update_episode_outcome_by_id(
                conn,
                &ep_id,
                outcome,
                confidence,
                experiment_id_owned.as_deref(),
                &verification_status,
                &search_text,
                &embedding_bytes,
                q8_bytes.as_deref(),
            )?;
            if let Some((weights, representation)) =
                sparse.as_ref().zip(sparse_representation.as_deref())
            {
                db::store_sparse_vector(conn, &episode_item_key(&ep_id), weights, representation)?;
            }
            Ok(())
        })
        .await?;

        #[cfg(feature = "hnsw")]
        self.sync_pending_hnsw_ops_best_effort("update_episode_outcome_by_id")
            .await;

        Ok(())
    }

    /// Update the outcome of an existing episode.
    pub async fn update_episode_outcome(
        &self,
        document_id: &str,
        outcome: EpisodeOutcome,
        confidence: f32,
        experiment_id: Option<&str>,
    ) -> Result<(), MemoryError> {
        Self::validate_confidence(confidence)?;
        let doc_id = document_id.to_string();
        let current_meta = self
            .with_read_conn(move |conn| load_episode_meta(conn, &doc_id))
            .await?
            .ok_or_else(|| MemoryError::DocumentNotFound(document_id.to_string()))?;

        let experiment_id_owned = experiment_id.map(|value| value.to_string());
        let verification_status =
            verification_status_for_outcome(&outcome, experiment_id_owned.as_deref());
        let updated_meta = EpisodeMeta {
            cause_ids: current_meta.cause_ids,
            effect_type: current_meta.effect_type,
            outcome: outcome.clone(),
            confidence,
            verification_status: verification_status.clone(),
            experiment_id: experiment_id_owned.clone().or(current_meta.experiment_id),
            valid_time: current_meta.valid_time,
            fact_digest: current_meta.fact_digest.clone(),
        };

        let doc_id = document_id.to_string();
        let (document_title, document_context) = self
            .with_read_conn(move |conn| load_episode_context(conn, &doc_id))
            .await?;
        let search_text =
            build_episode_search_text(&document_title, &document_context, &updated_meta);
        let (embedding, sparse, sparse_representation) = self
            .embed_text_with_sparse_internal(&search_text, crate::EmbeddingPurpose::Document)
            .await?;
        self.validate_embedding_dimensions(&embedding)?;
        let embedding_bytes = db::embedding_to_bytes(&embedding);
        // INTENTIONAL: q8 quantization is an optional search optimization; missing q8 is non-fatal
        let q8_bytes = Quantizer::new(self.inner.config.embedding.dimensions)
            .quantize(&embedding)
            .map(|vector| quantize::pack_quantized(&vector))
            .ok();

        let doc_id = document_id.to_string();
        self.with_write_conn(move |conn| {
            crate::episodes::update_episode_outcome(
                conn,
                &doc_id,
                outcome,
                confidence,
                experiment_id_owned.as_deref(),
                &verification_status,
                &search_text,
                &embedding_bytes,
                q8_bytes.as_deref(),
            )?;
            if let Some((weights, representation)) =
                sparse.as_ref().zip(sparse_representation.as_deref())
            {
                let episode_id: String = conn.query_row(
                    "SELECT episode_id FROM episodes WHERE document_id = ?1",
                    rusqlite::params![&doc_id],
                    |row| row.get(0),
                )?;
                db::store_sparse_vector(
                    conn,
                    &episode_item_key(&episode_id),
                    weights,
                    representation,
                )?;
            }
            Ok(())
        })
        .await?;

        #[cfg(feature = "hnsw")]
        self.sync_pending_hnsw_ops_best_effort("update_episode_outcome")
            .await;

        Ok(())
    }

    /// Search for episodes by effect_type and/or outcome.
    pub async fn search_episodes(
        &self,
        effect_type: Option<&str>,
        outcome: Option<&EpisodeOutcome>,
        limit: usize,
    ) -> Result<Vec<(String, EpisodeMeta)>, MemoryError> {
        let et = effect_type.map(|s| s.to_string());
        let outcome_owned = outcome.cloned();

        self.with_read_conn(move |conn| {
            search_episodes(conn, et.as_deref(), outcome_owned.as_ref(), limit)
        })
        .await
    }
}
