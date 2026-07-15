//! Fact CRUD with FTS5 synchronization.
//!
//! Every fact operation that touches `facts_fts` is transactional.

use crate::db;
use crate::db::{bytes_to_embedding, parse_optional_json, with_transaction};
#[cfg(feature = "hnsw")]
use crate::db::{enqueue_pending_index_op, PendingIndexOpKind};
#[cfg(feature = "hnsw")]
use crate::episodes;
use crate::error::MemoryError;
use crate::quantize::{self, Quantizer};
use crate::types::{Fact, NamespaceDeleteReport};
use crate::{merge_trace_ctx, MemoryStore};
use rusqlite::{params, Connection};
use stack_ids::TraceCtx;

/// Insert a fact and its FTS entry in a transaction.
#[allow(dead_code)]
pub fn insert_fact_with_fts(
    conn: &Connection,
    fact_id: &str,
    namespace: &str,
    content: &str,
    embedding_bytes: &[u8],
    source: Option<&str>,
    metadata: Option<&serde_json::Value>,
) -> Result<(), MemoryError> {
    insert_fact_with_fts_q8(
        conn,
        fact_id,
        namespace,
        content,
        embedding_bytes,
        None,
        source,
        metadata,
        None,
    )
}

/// Insert a fact with both f32 and quantized embeddings.
#[allow(clippy::too_many_arguments)]
pub fn insert_fact_with_fts_q8(
    conn: &Connection,
    fact_id: &str,
    namespace: &str,
    content: &str,
    embedding_bytes: &[u8],
    q8_bytes: Option<&[u8]>,
    source: Option<&str>,
    metadata: Option<&serde_json::Value>,
    sparse: Option<(&crate::SparseWeights, &str)>,
) -> Result<(), MemoryError> {
    let metadata_str = metadata.map(|m| m.to_string());
    with_transaction(conn, |tx| {
        tx.execute(
            "INSERT INTO facts (id, namespace, content, source, embedding, embedding_q8, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                fact_id,
                namespace,
                content,
                source,
                embedding_bytes,
                q8_bytes,
                metadata_str
            ],
        )?;

        tx.execute(
            "INSERT INTO facts_rowid_map (fact_id) VALUES (?1)",
            params![fact_id],
        )?;
        let fts_rowid = tx.last_insert_rowid();

        tx.execute(
            "INSERT INTO facts_fts(rowid, content) VALUES (?1, ?2)",
            params![fts_rowid, content],
        )?;

        #[cfg(feature = "hnsw")]
        enqueue_pending_index_op(
            tx,
            &format!("fact:{}", fact_id),
            "fact",
            PendingIndexOpKind::Upsert,
        )?;
        db::invalidate_derived_vector_artifact(tx, &format!("fact:{fact_id}"))?;
        if let Some((weights, representation)) = sparse {
            db::store_sparse_vector(tx, &format!("fact:{fact_id}"), weights, representation)?;
        }

        let changed = tx.execute(
            "UPDATE authority_state SET retrieval_epoch = retrieval_epoch + 1 WHERE id = 1",
            [],
        )?;
        if changed != 1 {
            return Err(MemoryError::Other(
                "failed to advance corpus authority epoch after fact insert".to_string(),
            ));
        }

        Ok(())
    })
}

/// Insert a fact within an existing transaction (no nested transaction).
///
/// Used by the import boundary where the outer transaction is already active.
#[allow(clippy::too_many_arguments)]
pub fn insert_fact_in_tx(
    tx: &rusqlite::Transaction<'_>,
    fact_id: &str,
    namespace: &str,
    content: &str,
    embedding_bytes: &[u8],
    q8_bytes: Option<&[u8]>,
    source: Option<&str>,
    metadata: Option<&serde_json::Value>,
) -> Result<(), MemoryError> {
    let metadata_str = metadata.map(|m| m.to_string());
    tx.execute(
        "INSERT INTO facts (id, namespace, content, source, embedding, embedding_q8, metadata)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            fact_id,
            namespace,
            content,
            source,
            embedding_bytes,
            q8_bytes,
            metadata_str
        ],
    )?;

    tx.execute(
        "INSERT INTO facts_rowid_map (fact_id) VALUES (?1)",
        params![fact_id],
    )?;
    let fts_rowid = tx.last_insert_rowid();

    tx.execute(
        "INSERT INTO facts_fts(rowid, content) VALUES (?1, ?2)",
        params![fts_rowid, content],
    )?;

    #[cfg(feature = "hnsw")]
    enqueue_pending_index_op(
        tx,
        &format!("fact:{}", fact_id),
        "fact",
        PendingIndexOpKind::Upsert,
    )?;
    db::invalidate_derived_vector_artifact(tx, &format!("fact:{fact_id}"))?;

    Ok(())
}

/// Delete a fact and its FTS entry in a transaction.
#[allow(dead_code)] // public API — used by external consumers, not internally
pub fn delete_fact_with_fts(conn: &Connection, fact_id: &str) -> Result<(), MemoryError> {
    with_transaction(conn, |tx| {
        let fts_rowid: i64 = tx
            .query_row(
                "SELECT rowid FROM facts_rowid_map WHERE fact_id = ?1",
                params![fact_id],
                |row| row.get(0),
            )
            .map_err(|e| MemoryError::FactNotFound(format!("{}: {e}", fact_id)))?;

        let content: String = tx
            .query_row(
                "SELECT content FROM facts WHERE id = ?1",
                params![fact_id],
                |row| row.get(0),
            )
            .map_err(|e| MemoryError::FactNotFound(format!("{}: {e}", fact_id)))?;

        tx.execute(
            "INSERT INTO facts_fts(facts_fts, rowid, content) VALUES('delete', ?1, ?2)",
            params![fts_rowid, content],
        )?;
        tx.execute(
            "DELETE FROM facts_rowid_map WHERE fact_id = ?1",
            params![fact_id],
        )?;
        tx.execute(
            "DELETE FROM episode_causes WHERE cause_node_id IN (?1, ?2)",
            params![fact_id, format!("fact:{fact_id}")],
        )?;
        tx.execute(
            "DELETE FROM derivation_edges
             WHERE (source_kind = 'fact' AND source_id = ?1)
                OR (target_kind = 'fact' AND target_id = ?1)",
            params![fact_id],
        )?;
        tx.execute("DELETE FROM facts WHERE id = ?1", params![fact_id])?;

        #[cfg(feature = "hnsw")]
        enqueue_pending_index_op(
            tx,
            &format!("fact:{}", fact_id),
            "fact",
            PendingIndexOpKind::Delete,
        )?;
        db::invalidate_derived_vector_artifact(tx, &format!("fact:{fact_id}"))?;

        Ok(())
    })
}

/// Update a fact's content and embeddings, with FTS synchronization.
#[allow(dead_code)] // public API — used by external consumers, not internally
pub fn update_fact_with_fts(
    conn: &Connection,
    fact_id: &str,
    new_content: &str,
    new_embedding_bytes: &[u8],
    new_q8_bytes: Option<&[u8]>,
) -> Result<(), MemoryError> {
    with_transaction(conn, |tx| {
        let (fts_rowid, old_content): (i64, String) = tx
            .query_row(
                "SELECT fm.rowid, f.content
                 FROM facts f
                 JOIN facts_rowid_map fm ON fm.fact_id = f.id
                 WHERE f.id = ?1",
                params![fact_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| MemoryError::FactNotFound(format!("{}: {e}", fact_id)))?;

        tx.execute(
            "INSERT INTO facts_fts(facts_fts, rowid, content) VALUES('delete', ?1, ?2)",
            params![fts_rowid, old_content],
        )?;

        tx.execute(
            "UPDATE facts
             SET content = ?1,
                 embedding = ?2,
                 embedding_q8 = ?3,
                 updated_at = datetime('now')
             WHERE id = ?4",
            params![new_content, new_embedding_bytes, new_q8_bytes, fact_id],
        )?;

        tx.execute(
            "INSERT INTO facts_fts(rowid, content) VALUES (?1, ?2)",
            params![fts_rowid, new_content],
        )?;
        tx.execute(
            "DELETE FROM derivation_edges
             WHERE (source_kind = 'fact' AND source_id = ?1)
                OR (target_kind = 'fact' AND target_id = ?1)",
            params![fact_id],
        )?;

        #[cfg(feature = "hnsw")]
        enqueue_pending_index_op(
            tx,
            &format!("fact:{}", fact_id),
            "fact",
            PendingIndexOpKind::Upsert,
        )?;
        db::invalidate_derived_vector_artifact(tx, &format!("fact:{fact_id}"))?;

        Ok(())
    })
}

/// Delete all namespace-scoped memory atomically and report every affected surface.
#[cfg(feature = "admin-ops")]
pub fn delete_namespace(
    conn: &Connection,
    namespace: &str,
) -> Result<NamespaceDeleteReport, MemoryError> {
    with_transaction(conn, |tx| {
        let mut report = NamespaceDeleteReport::default();
        let delete_session = |session_id: &str| -> Result<(usize, usize), MemoryError> {
            let message_data: Vec<(i64, String, i64, bool)> = {
                let mut stmt = tx.prepare(
                    "SELECT m.id, m.content, mm.rowid, m.embedding IS NOT NULL
                     FROM messages m
                     JOIN messages_rowid_map mm ON mm.message_id = m.id
                     WHERE m.session_id = ?1",
                )?;
                let rows = stmt.query_map(params![session_id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?;
                rows.collect::<Result<Vec<_>, _>>()?
            };

            for (message_id, content, fts_rowid, has_embedding) in &message_data {
                #[cfg(not(feature = "hnsw"))]
                let _ = (message_id, has_embedding);
                tx.execute(
                    "INSERT INTO messages_fts(messages_fts, rowid, content) VALUES('delete', ?1, ?2)",
                    params![fts_rowid, content],
                )?;
                #[cfg(feature = "hnsw")]
                if *has_embedding {
                    enqueue_pending_index_op(
                        tx,
                        &format!("msg:{}", message_id),
                        "message",
                        PendingIndexOpKind::Delete,
                    )?;
                }
            }

            let affected = tx.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
            if affected == 0 {
                return Err(MemoryError::SessionNotFound(session_id.to_string()));
            }
            let hnsw_ops = message_data
                .iter()
                .filter(|(_, _, _, has_embedding)| *has_embedding)
                .count();
            Ok((message_data.len(), hnsw_ops))
        };

        let document_ids: Vec<String> = {
            let mut stmt = tx.prepare("SELECT id FROM documents WHERE namespace = ?1")?;
            let ids = stmt
                .query_map(params![namespace], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids
        };

        let session_ids: Vec<String> = {
            let mut stmt = tx.prepare("SELECT id, metadata FROM sessions")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?;
            let mut ids = Vec::new();
            for row in rows {
                let (session_id, metadata_raw) = row?;
                let metadata = parse_optional_json(
                    "sessions",
                    &session_id,
                    "metadata",
                    metadata_raw.as_deref(),
                )?;
                let namespace_matches = metadata
                    .as_ref()
                    .and_then(|value| {
                        value
                            .get("namespace")
                            .or_else(|| value.get("scope_namespace"))
                    })
                    .and_then(|value| value.as_str())
                    == Some(namespace);
                if namespace_matches {
                    ids.push(session_id);
                }
            }
            ids
        };

        for session_id in &session_ids {
            let (messages, hnsw_ops) = delete_session(session_id)?;
            report.messages += messages;
            report.hnsw_ops += hnsw_ops;
        }
        report.sessions = session_ids.len();

        let delete_derivation_edges_for_id = |kind: &str, id: &str| -> Result<(), MemoryError> {
            tx.execute(
                "DELETE FROM derivation_edges
                 WHERE (source_kind = ?1 AND source_id = ?2)
                    OR (target_kind = ?1 AND target_id = ?2)",
                params![kind, id],
            )?;
            Ok(())
        };

        let delete_derivation_edges_for_ids =
            |kind: &str, ids: &[String]| -> Result<(), MemoryError> {
                for id in ids {
                    delete_derivation_edges_for_id(kind, id)?;
                }
                Ok(())
            };

        let facts: Vec<(String, i64, String)> = {
            let mut stmt = tx.prepare(
                "SELECT f.id, fm.rowid, f.content
                 FROM facts f
                 JOIN facts_rowid_map fm ON fm.fact_id = f.id
                 WHERE f.namespace = ?1",
            )?;
            let facts = stmt
                .query_map(params![namespace], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            facts
        };

        for (fact_id, fts_rowid, content) in &facts {
            tx.execute(
                "INSERT INTO facts_fts(facts_fts, rowid, content) VALUES('delete', ?1, ?2)",
                params![fts_rowid, content],
            )?;
            tx.execute(
                "DELETE FROM facts_rowid_map WHERE fact_id = ?1",
                params![fact_id],
            )?;

            #[cfg(feature = "hnsw")]
            enqueue_pending_index_op(
                tx,
                &format!("fact:{}", fact_id),
                "fact",
                PendingIndexOpKind::Delete,
            )?;
            #[cfg(feature = "hnsw")]
            {
                report.hnsw_ops += 1;
            }
        }
        tx.execute("DELETE FROM facts WHERE namespace = ?1", params![namespace])?;
        report.facts = facts.len();

        for doc_id in &document_ids {
            let mut stmt = tx.prepare(
                "SELECT c.id, c.content, cm.rowid
                 FROM chunks c
                 JOIN chunks_rowid_map cm ON cm.chunk_id = c.id
                 WHERE c.document_id = ?1",
            )?;
            let chunk_rows: Vec<(String, String, i64)> = stmt
                .query_map(params![doc_id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            report.chunks += chunk_rows.len();

            for (chunk_id, content, fts_rowid) in &chunk_rows {
                tx.execute(
                    "INSERT INTO chunks_fts(chunks_fts, rowid, content) VALUES ('delete', ?1, ?2)",
                    params![fts_rowid, content],
                )?;
                tx.execute(
                    "DELETE FROM chunks_rowid_map WHERE chunk_id = ?1",
                    params![chunk_id],
                )?;
                #[cfg(feature = "hnsw")]
                enqueue_pending_index_op(
                    tx,
                    &format!("chunk:{}", chunk_id),
                    "chunk",
                    PendingIndexOpKind::Delete,
                )?;
                #[cfg(feature = "hnsw")]
                {
                    report.hnsw_ops += 1;
                }
            }

            tx.execute("DELETE FROM chunks WHERE document_id = ?1", params![doc_id])?;
        }

        for doc_id in &document_ids {
            let mut stmt = tx.prepare(
                "SELECT e.episode_id, e.search_text, erm.rowid
                 FROM episodes e
                 JOIN episodes_rowid_map erm ON erm.episode_id = e.episode_id
                 WHERE e.document_id = ?1",
            )?;
            let episode_rows: Vec<(String, String, i64)> = stmt
                .query_map(params![doc_id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            report.episodes += episode_rows.len();

            for (episode_id, search_text, fts_rowid) in &episode_rows {
                tx.execute(
                    "INSERT INTO episodes_fts(episodes_fts, rowid, content) VALUES ('delete', ?1, ?2)",
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
                enqueue_pending_index_op(
                    tx,
                    &episodes::episode_item_key(episode_id),
                    "episode",
                    PendingIndexOpKind::Delete,
                )?;
                #[cfg(feature = "hnsw")]
                {
                    report.hnsw_ops += 1;
                }
            }

            tx.execute(
                "DELETE FROM episodes WHERE document_id = ?1",
                params![doc_id],
            )?;
            tx.execute("DELETE FROM documents WHERE id = ?1", params![doc_id])?;
        }
        report.documents = document_ids.len();

        let claim_ids: Vec<String> = {
            let mut stmt =
                tx.prepare("SELECT claim_id FROM claim_versions WHERE scope_namespace = ?1")?;
            let ids = stmt
                .query_map(params![namespace], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids
        };

        let claim_version_ids: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT claim_version_id FROM claim_versions WHERE scope_namespace = ?1",
            )?;
            let ids = stmt
                .query_map(params![namespace], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids
        };

        let relation_version_ids: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT relation_version_id FROM relation_versions WHERE scope_namespace = ?1",
            )?;
            let ids = stmt
                .query_map(params![namespace], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids
        };

        let alias_entity_ids: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT canonical_entity_id FROM entity_aliases WHERE scope_namespace = ?1",
            )?;
            let ids = stmt
                .query_map(params![namespace], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids
        };

        let evidence_handles: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT er.fetch_handle FROM evidence_refs er
                 JOIN projection_import_log pil ON er.source_envelope_id = pil.source_envelope_id
                 WHERE pil.scope_namespace = ?1",
            )?;
            let handles = stmt
                .query_map(params![namespace], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            handles
        };

        let episode_ids: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT episode_id FROM episode_links
                 WHERE source_envelope_id IN (SELECT source_envelope_id FROM projection_import_log WHERE scope_namespace = ?1)",
            )?;
            let ids = stmt
                .query_map(params![namespace], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids
        };

        delete_derivation_edges_for_ids("claim", &claim_ids)?;
        delete_derivation_edges_for_ids("claim_version", &claim_version_ids)?;
        delete_derivation_edges_for_ids("relation_version", &relation_version_ids)?;
        delete_derivation_edges_for_ids("entity", &alias_entity_ids)?;
        delete_derivation_edges_for_ids("evidence_ref", &evidence_handles)?;
        delete_derivation_edges_for_ids("episode", &episode_ids)?;

        report.projection_rows += tx.execute(
            "DELETE FROM claim_versions WHERE scope_namespace = ?1",
            params![namespace],
        )?;
        report.projection_rows += tx.execute(
            "DELETE FROM relation_versions WHERE scope_namespace = ?1",
            params![namespace],
        )?;
        report.projection_rows += tx.execute(
            "DELETE FROM entity_aliases WHERE scope_namespace = ?1",
            params![namespace],
        )?;
        report.projection_rows += tx.execute(
            "DELETE FROM evidence_refs
             WHERE source_envelope_id IN (SELECT source_envelope_id FROM projection_import_log WHERE scope_namespace = ?1)",
            params![namespace],
        )?;
        report.projection_rows += tx.execute(
            "DELETE FROM episode_links
             WHERE source_envelope_id IN (SELECT source_envelope_id FROM projection_import_log WHERE scope_namespace = ?1)",
            params![namespace],
        )?;
        report.projection_rows += tx.execute(
            "DELETE FROM projection_import_failures WHERE scope_namespace = ?1",
            params![namespace],
        )?;
        report.projection_rows += tx.execute(
            "DELETE FROM projection_import_log WHERE scope_namespace = ?1",
            params![namespace],
        )?;

        Ok(report)
    })
}

/// Get a fact by ID.
pub fn get_fact(conn: &Connection, fact_id: &str) -> Result<Option<Fact>, MemoryError> {
    let result = conn.query_row(
        "SELECT id, namespace, content, source, created_at, updated_at, metadata
         FROM facts WHERE id = ?1",
        params![fact_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        },
    );

    match result {
        Ok((id, namespace, content, source, created_at, updated_at, metadata_raw)) => {
            Ok(Some(Fact {
                metadata: parse_optional_json("facts", &id, "metadata", metadata_raw.as_deref())?,
                id,
                namespace,
                content,
                source,
                created_at,
                updated_at,
            }))
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(MemoryError::Database(err)),
    }
}

/// Get a fact embedding vector.
pub fn get_fact_embedding(
    conn: &Connection,
    fact_id: &str,
) -> Result<Option<Vec<f32>>, MemoryError> {
    let result: Result<Option<Vec<u8>>, _> = conn.query_row(
        "SELECT embedding FROM facts WHERE id = ?1",
        params![fact_id],
        |row| row.get(0),
    );

    match result {
        Ok(Some(bytes)) => Ok(Some(bytes_to_embedding(&bytes)?)),
        Ok(None) => Ok(None),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(MemoryError::Database(err)),
    }
}

/// List the distinct namespaces that currently contain facts.
pub fn list_fact_namespaces(conn: &Connection) -> Result<Vec<String>, MemoryError> {
    let mut stmt = conn.prepare("SELECT DISTINCT namespace FROM facts ORDER BY namespace")?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// List facts within a namespace.
#[allow(dead_code)] // retained as an internal compatibility seam for older callers
pub fn list_facts(
    conn: &Connection,
    namespace: &str,
    limit: usize,
    offset: usize,
) -> Result<Vec<Fact>, MemoryError> {
    list_facts_with_view(conn, namespace, limit, offset, &StateView::Current)
}

/// Authority state selected by a fact retrieval.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StateView {
    Current,
    HistoricalAt(String),
    RecordedAsOf(String),
    IncludeSuperseded,
}

pub(crate) fn fact_is_visible_with_view(
    conn: &Connection,
    fact_id: &str,
    view: &StateView,
) -> Result<bool, MemoryError> {
    let forgotten: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM forgotten_facts WHERE fact_id = ?1)",
        params![fact_id],
        |row| row.get(0),
    )?;
    if forgotten {
        return Ok(false);
    }
    let cutoff = match view {
        StateView::HistoricalAt(value) | StateView::RecordedAsOf(value) => {
            let parsed = chrono::DateTime::parse_from_rfc3339(value).map_err(|e| {
                MemoryError::Other(format!("invalid StateView timestamp '{value}': {e}"))
            })?;
            Some(
                parsed
                    .with_timezone(&chrono::Utc)
                    .format("%Y-%m-%d %H:%M:%S%.6f")
                    .to_string(),
            )
        }
        _ => None,
    };
    let include_superseded = matches!(view, StateView::IncludeSuperseded);
    let visible: i64 = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM facts f
             WHERE f.id = ?1
               AND (?2 IS NULL OR f.created_at <= ?2)
               AND (?3 = 1 OR NOT EXISTS (
                   SELECT 1 FROM graph_edges ge
                   WHERE ge.target = 'fact:' || f.id
                     AND ge.is_invalidated = 0
                     AND COALESCE(
                         json_extract(ge.edge_type, '$.relation'),
                         json_extract(ge.edge_type, '$.entity.relation')
                     ) IN ('supersedes', 'redacts')
                     AND (?2 IS NULL OR COALESCE(ge.recorded_time, ge.recorded_at) <= ?2)
               ))
         )",
        params![fact_id, cutoff, include_superseded],
        |row| row.get(0),
    )?;
    Ok(visible != 0)
}

/// List facts under an explicit authority-state view. Inconsistent lineage is rejected.
pub fn list_facts_with_view(
    conn: &Connection,
    namespace: &str,
    limit: usize,
    offset: usize,
    view: &StateView,
) -> Result<Vec<Fact>, MemoryError> {
    let cutoff = match view {
        StateView::HistoricalAt(value) | StateView::RecordedAsOf(value) => {
            let parsed = chrono::DateTime::parse_from_rfc3339(value).map_err(|e| {
                MemoryError::Other(format!("invalid StateView timestamp '{value}': {e}"))
            })?;
            Some(
                parsed
                    .with_timezone(&chrono::Utc)
                    .format("%Y-%m-%d %H:%M:%S%.6f")
                    .to_string(),
            )
        }
        _ => None,
    };
    let inconsistent: i64 = conn.query_row(
        "SELECT COUNT(*) FROM (
             SELECT target FROM graph_edges
             WHERE is_invalidated = 0
               AND COALESCE(
                   json_extract(edge_type, '$.relation'),
                   json_extract(edge_type, '$.entity.relation')
               ) IN ('supersedes', 'redacts')
               AND (?1 IS NULL OR COALESCE(recorded_time, recorded_at) <= ?1)
             GROUP BY target HAVING COUNT(DISTINCT source) > 1
         )",
        params![cutoff.as_deref()],
        |row| row.get(0),
    )?;
    if inconsistent != 0 {
        return Err(MemoryError::Other(
            "inconsistent fact lineage: multiple active heads".into(),
        ));
    }
    let include_superseded = matches!(view, StateView::IncludeSuperseded);
    let mut stmt = conn.prepare(
        "SELECT id, namespace, content, source, created_at, updated_at, metadata
         FROM facts
         WHERE namespace = ?1
           AND NOT EXISTS (
               SELECT 1 FROM forgotten_facts ff WHERE ff.fact_id = facts.id
           )
           AND (?4 IS NULL OR created_at <= ?4)
           AND (?5 = 1 OR NOT EXISTS (
               SELECT 1 FROM graph_edges ge
               WHERE ge.target = 'fact:' || facts.id
                 AND ge.is_invalidated = 0
                 AND COALESCE(
                     json_extract(ge.edge_type, '$.relation'),
                     json_extract(ge.edge_type, '$.entity.relation')
                 ) IN ('supersedes', 'redacts')
                 AND (?4 IS NULL OR COALESCE(ge.recorded_time, ge.recorded_at) <= ?4)
           ))
         ORDER BY updated_at DESC
         LIMIT ?2 OFFSET ?3",
    )?;

    let facts = stmt
        .query_map(
            params![
                namespace,
                limit as i64,
                offset as i64,
                cutoff,
                include_superseded
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(
            |(id, namespace, content, source, created_at, updated_at, metadata_raw)| {
                Ok(Fact {
                    metadata: parse_optional_json(
                        "facts",
                        &id,
                        "metadata",
                        metadata_raw.as_deref(),
                    )?,
                    id,
                    namespace,
                    content,
                    source,
                    created_at,
                    updated_at,
                })
            },
        )
        .collect::<Result<Vec<_>, MemoryError>>()?;

    Ok(facts)
}

impl MemoryStore {
    /// Explicitly ungoverned compatibility write.
    ///
    /// This preserves the pre-authority raw storage API for migrations and local tooling. It does
    /// not create an origin label and its output is therefore denied by every governed path.
    pub async fn add_fact_raw_compat(
        &self,
        namespace: &str,
        content: &str,
        source: Option<&str>,
        metadata: Option<serde_json::Value>,
        trace_ctx: Option<&TraceCtx>,
    ) -> Result<Fact, MemoryError> {
        let id = self
            .add_fact_with_trace(namespace, content, source, metadata, trace_ctx)
            .await?;
        self.get_fact(&id)
            .await?
            .ok_or(MemoryError::FactNotFound(id))
    }

    /// Store a fact with automatic embedding. Returns the fact ID (UUID v4).
    ///
    /// This is a non-authoritative storage primitive. Governed mutations must
    /// use [`MemoryStore::authority`] so admission and lineage are enforced.
    pub async fn add_fact(
        &self,
        namespace: &str,
        content: &str,
        source: Option<&str>,
        metadata: Option<serde_json::Value>,
    ) -> Result<String, MemoryError> {
        self.add_fact_with_trace(namespace, content, source, metadata, None)
            .await
    }

    /// Store a fact with automatic embedding and optional trace metadata.
    pub async fn add_fact_with_trace(
        &self,
        namespace: &str,
        content: &str,
        source: Option<&str>,
        metadata: Option<serde_json::Value>,
        trace_ctx: Option<&TraceCtx>,
    ) -> Result<String, MemoryError> {
        self.validate_content("fact.content", content)?;

        // Dedup: check if a fact with the same content already exists.
        // This prevents the 4-5% DB bloat from duplicate ingestion.
        let ns_check = namespace.to_string();
        let ct_check = content.to_string();
        let existing_id = self
            .with_read_conn(move |conn| {
                let result: Option<String> = conn
                    .query_row(
                        "SELECT id FROM facts WHERE content = ?1 AND namespace = ?2 LIMIT 1",
                        rusqlite::params![&ct_check, &ns_check],
                        |row| row.get::<_, String>(0),
                    )
                    .ok();
                Ok(result)
            })
            .await?;

        if let Some(id) = existing_id {
            return Ok(id);
        }

        let (embedding, sparse, sparse_representation) = self
            .embed_text_with_sparse_internal(content, crate::EmbeddingPurpose::Document)
            .await?;
        self.validate_embedding_dimensions(&embedding)?;
        let embedding_bytes = db::embedding_to_bytes(&embedding);
        let fact_id = uuid::Uuid::new_v4().to_string();
        let max_facts_per_namespace = self.inner.config.limits.max_facts_per_namespace;

        let quantizer = Quantizer::new(self.inner.config.embedding.dimensions);
        // INTENTIONAL: q8 quantization is an optional search optimization; missing q8 is non-fatal
        let q8_bytes = quantizer
            .quantize(&embedding)
            .map(|qv| quantize::pack_quantized(&qv))
            .ok();

        let ns = namespace.to_string();
        let ct = content.to_string();
        let fid = fact_id.clone();
        let src = source.map(|s| s.to_string());
        let meta = merge_trace_ctx(metadata, trace_ctx);
        self.with_write_conn(move |conn| {
            let current_count: usize = conn.query_row(
                "SELECT COUNT(*) FROM facts WHERE namespace = ?1",
                rusqlite::params![&ns],
                |row| row.get(0),
            )?;
            if current_count >= max_facts_per_namespace {
                return Err(MemoryError::NamespaceFull {
                    namespace: ns.clone(),
                    count: current_count,
                    limit: max_facts_per_namespace,
                });
            }
            insert_fact_with_fts_q8(
                conn,
                &fid,
                &ns,
                &ct,
                &embedding_bytes,
                q8_bytes.as_deref(),
                src.as_deref(),
                meta.as_ref(),
                sparse.as_ref().zip(sparse_representation.as_deref()),
            )
        })
        .await?;

        self.clear_search_cache();

        #[cfg(feature = "hnsw")]
        self.sync_pending_hnsw_ops_best_effort("add_fact").await;

        Ok(fact_id)
    }

    /// Store a fact with a pre-computed embedding.
    pub async fn add_fact_with_embedding(
        &self,
        namespace: &str,
        content: &str,
        embedding: &[f32],
        source: Option<&str>,
        metadata: Option<serde_json::Value>,
    ) -> Result<String, MemoryError> {
        self.add_fact_with_embedding_and_trace(
            namespace, content, embedding, source, metadata, None,
        )
        .await
    }

    /// Store a fact with a pre-computed embedding and optional trace metadata.
    pub async fn add_fact_with_embedding_and_trace(
        &self,
        namespace: &str,
        content: &str,
        embedding: &[f32],
        source: Option<&str>,
        metadata: Option<serde_json::Value>,
        trace_ctx: Option<&TraceCtx>,
    ) -> Result<String, MemoryError> {
        self.validate_content("fact.content", content)?;
        self.validate_embedding_dimensions(embedding)?;
        let embedding_bytes = db::embedding_to_bytes(embedding);
        let sparse = self.inner.config.search.derive_sparse_from_dense.then(|| {
            crate::SparseWeights::from_dense(
                embedding,
                self.inner.config.search.sparse_derive_top_k,
                self.inner.config.search.sparse_derive_min_weight,
            )
        });
        let fact_id = uuid::Uuid::new_v4().to_string();
        let max_facts_per_namespace = self.inner.config.limits.max_facts_per_namespace;

        let quantizer = Quantizer::new(self.inner.config.embedding.dimensions);
        // INTENTIONAL: q8 quantization is an optional search optimization; missing q8 is non-fatal
        let q8_bytes = quantizer
            .quantize(embedding)
            .map(|qv| quantize::pack_quantized(&qv))
            .ok();

        let ns = namespace.to_string();
        let ct = content.to_string();
        let fid = fact_id.clone();
        let src = source.map(|s| s.to_string());
        let meta = merge_trace_ctx(metadata, trace_ctx);
        self.with_write_conn(move |conn| {
            let current_count: usize = conn.query_row(
                "SELECT COUNT(*) FROM facts WHERE namespace = ?1",
                rusqlite::params![&ns],
                |row| row.get(0),
            )?;
            if current_count >= max_facts_per_namespace {
                return Err(MemoryError::NamespaceFull {
                    namespace: ns.clone(),
                    count: current_count,
                    limit: max_facts_per_namespace,
                });
            }
            insert_fact_with_fts_q8(
                conn,
                &fid,
                &ns,
                &ct,
                &embedding_bytes,
                q8_bytes.as_deref(),
                src.as_deref(),
                meta.as_ref(),
                sparse
                    .as_ref()
                    .map(|weights| (weights, "generic_dense_derived_sparse")),
            )
        })
        .await?;

        self.clear_search_cache();

        #[cfg(feature = "hnsw")]
        self.sync_pending_hnsw_ops_best_effort("add_fact_with_embedding")
            .await;

        Ok(fact_id)
    }

    /// **DANGER**: This physically mutates/deletes a truth-bearing row.
    /// This is admin-only and gated behind the `admin-ops` feature.
    /// Default agent-facing APIs should use supersession (add a new fact
    /// with a supersession link) instead of hard delete/update.
    #[cfg(feature = "admin-ops")]
    pub async fn update_fact(&self, fact_id: &str, content: &str) -> Result<(), MemoryError> {
        self.validate_content("fact.content", content)?;
        let (embedding, sparse, sparse_representation) = self
            .embed_text_with_sparse_internal(content, crate::EmbeddingPurpose::Document)
            .await?;
        self.validate_embedding_dimensions(&embedding)?;
        let embedding_bytes = db::embedding_to_bytes(&embedding);
        // INTENTIONAL: q8 quantization is an optional search optimization; missing q8 is non-fatal
        let q8_bytes = Quantizer::new(self.inner.config.embedding.dimensions)
            .quantize(&embedding)
            .map(|qv| quantize::pack_quantized(&qv))
            .ok();

        let fid = fact_id.to_string();
        let ct = content.to_string();
        self.with_write_conn(move |conn| {
            update_fact_with_fts(conn, &fid, &ct, &embedding_bytes, q8_bytes.as_deref())?;
            let item_key = format!("fact:{fid}");
            if let Some((weights, representation)) =
                sparse.as_ref().zip(sparse_representation.as_deref())
            {
                db::store_sparse_vector(conn, &item_key, weights, representation)?;
            } else {
                db::delete_sparse_vector(conn, &item_key)?;
            }
            Ok(())
        })
        .await?;

        #[cfg(feature = "hnsw")]
        self.sync_pending_hnsw_ops_best_effort("update_fact").await;

        self.clear_search_cache();

        Ok(())
    }

    /// **DANGER**: This physically mutates/deletes a truth-bearing row.
    /// This is admin-only and gated behind the `admin-ops` feature.
    /// Default agent-facing APIs should use supersession (add a new fact
    /// with a supersession link) instead of hard delete/update.
    #[cfg(feature = "admin-ops")]
    pub async fn delete_fact(&self, fact_id: &str) -> Result<(), MemoryError> {
        let fid = fact_id.to_string();
        self.with_write_conn(move |conn| delete_fact_with_fts(conn, &fid))
            .await?;

        #[cfg(feature = "hnsw")]
        self.sync_pending_hnsw_ops_best_effort("delete_fact").await;

        self.clear_search_cache();

        Ok(())
    }

    #[cfg(not(feature = "admin-ops"))]
    #[doc = r#"
    ```compile_fail
    use semantic_memory::MemoryStore;

    fn main() {
        let _ = MemoryStore::update_fact;
        let _ = MemoryStore::delete_fact;
        let _ = MemoryStore::delete_namespace;
        let _ = MemoryStore::reembed_all;
    }
    ```"#]
    pub(crate) const _MEM014_ADMIN_OPS_GUARD: () = ();

    /// **DANGER**: physically deletes every truth-bearing row in a namespace.
    /// This is admin-only and gated behind the `admin-ops` feature. Ordinary
    /// callers must use governed supersession/forgetting flows instead.
    #[cfg(feature = "admin-ops")]
    pub async fn delete_namespace(
        &self,
        namespace: &str,
    ) -> Result<NamespaceDeleteReport, MemoryError> {
        let ns = namespace.to_string();
        let count = self
            .with_write_conn(move |conn| delete_namespace(conn, &ns))
            .await?;

        #[cfg(feature = "hnsw")]
        self.sync_pending_hnsw_ops_best_effort("delete_namespace")
            .await;

        self.clear_search_cache();

        Ok(count)
    }

    /// Get a fact by ID.
    pub async fn get_fact(&self, fact_id: &str) -> Result<Option<Fact>, MemoryError> {
        let fid = fact_id.to_string();
        self.with_read_conn(move |conn| get_fact(conn, &fid)).await
    }

    /// Explicitly ungoverned compatibility read. Prefer `authority().get_fact_governed`.
    pub async fn get_fact_raw_compat(&self, fact_id: &str) -> Result<Option<Fact>, MemoryError> {
        self.get_fact(fact_id).await
    }

    /// Get a fact's embedding vector.
    pub async fn get_fact_embedding(&self, fact_id: &str) -> Result<Option<Vec<f32>>, MemoryError> {
        let fid = fact_id.to_string();
        self.with_read_conn(move |conn| get_fact_embedding(conn, &fid))
            .await
    }

    /// List all facts in a namespace using the default `Current` view.
    pub async fn list_facts(
        &self,
        namespace: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Fact>, MemoryError> {
        self.list_facts_with_view(namespace, limit, offset, StateView::Current)
            .await
    }

    /// List facts under an explicit bitemporal authority-state view.
    pub async fn list_facts_with_view(
        &self,
        namespace: &str,
        limit: usize,
        offset: usize,
        view: StateView,
    ) -> Result<Vec<Fact>, MemoryError> {
        let ns = namespace.to_string();
        self.with_read_conn(move |conn| list_facts_with_view(conn, &ns, limit, offset, &view))
            .await
    }

    /// List the distinct namespaces that currently contain facts.
    pub async fn list_fact_namespaces(&self) -> Result<Vec<String>, MemoryError> {
        self.with_read_conn(move |conn| list_fact_namespaces(conn))
            .await
    }
}

#[cfg(test)]
mod state_view_regression_tests {
    use super::*;
    use crate::db::run_migrations;
    use rusqlite::Connection;

    fn seeded() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        for (id, content, created) in [
            ("old", "same topic old", "2026-07-10 21:00:00"),
            ("new", "same topic new", "2026-07-10 21:12:01"),
        ] {
            conn.execute(
                "INSERT INTO facts(id, namespace, content, created_at, updated_at) VALUES (?1, 'n', ?2, ?3, ?3)",
                params![id, content, created],
            ).unwrap();
        }
        conn
    }

    fn supersedes(conn: &Connection, source: &str, target: &str, recorded: &str) {
        conn.execute(
            "INSERT INTO graph_edges(id, source, target, edge_type, weight, content_digest, recorded_at, valid_time, recorded_time)
             VALUES (lower(hex(randomblob(16))), ?1, ?2, '{\"type\":\"entity\",\"relation\":\"supersedes\"}', 1, lower(hex(randomblob(16))), ?3, ?3, ?3)",
            params![format!("fact:{source}"), format!("fact:{target}"), recorded],
        ).unwrap();
    }

    fn supersedes_canonical(conn: &Connection, source: &str, target: &str, recorded: &str) {
        conn.execute(
            "INSERT INTO graph_edges(id, source, target, edge_type, weight, content_digest, recorded_at, valid_time, recorded_time)
             VALUES (lower(hex(randomblob(16))), ?1, ?2, '{\"entity\":{\"relation\":\"supersedes\"}}', 1, lower(hex(randomblob(16))), ?3, ?3, ?3)",
            params![format!("fact:{source}"), format!("fact:{target}"), recorded],
        ).unwrap();
    }

    #[test]
    fn historical_view_excludes_future_fact_and_reconstructs_pre_supersession_head() {
        let conn = seeded();
        supersedes(&conn, "new", "old", "2026-07-10 21:12:01");
        let rows = list_facts_with_view(
            &conn,
            "n",
            10,
            0,
            &StateView::HistoricalAt("2026-07-10T21:11:50Z".into()),
        )
        .unwrap();
        assert_eq!(
            rows.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            ["old"]
        );
    }

    #[test]
    fn historical_view_preserves_pre_adjudication_conflict() {
        let conn = seeded();
        conn.execute(
            "UPDATE facts SET created_at = '2026-07-10 21:10:00', updated_at = '2026-07-10 21:10:00' WHERE id = 'new'",
            [],
        )
        .unwrap();
        supersedes(&conn, "new", "old", "2026-07-10 21:12:01");

        let rows = list_facts_with_view(
            &conn,
            "n",
            10,
            0,
            &StateView::HistoricalAt("2026-07-10T21:11:50Z".into()),
        )
        .unwrap();
        let ids = rows.iter().map(|fact| fact.id.as_str()).collect::<Vec<_>>();
        assert!(
            ids.contains(&"old"),
            "prior observation must remain visible"
        );
        assert!(ids.contains(&"new"), "conflicting observation created before the cutoff must remain visible until adjudication");
    }

    #[test]
    fn current_view_excludes_superseded_fact() {
        let conn = seeded();
        supersedes(&conn, "new", "old", "2026-07-10 21:12:01");
        let rows = list_facts_with_view(&conn, "n", 10, 0, &StateView::Current).unwrap();
        assert_eq!(
            rows.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            ["new"]
        );
    }

    #[test]
    fn current_view_accepts_canonical_entity_edge_serialization() {
        let conn = seeded();
        supersedes_canonical(&conn, "new", "old", "2026-07-10 21:12:01");
        let rows = list_facts_with_view(&conn, "n", 10, 0, &StateView::Current).unwrap();
        assert_eq!(
            rows.iter().map(|fact| fact.id.as_str()).collect::<Vec<_>>(),
            vec!["new"]
        );
    }

    #[test]
    fn multiple_active_heads_fail_closed() {
        let conn = seeded();
        conn.execute("INSERT INTO facts(id, namespace, content, created_at, updated_at) VALUES ('other', 'n', 'same topic conflicting', '2026-07-10 21:13:00', '2026-07-10 21:13:00')", []).unwrap();
        supersedes(&conn, "new", "old", "2026-07-10 21:12:01");
        supersedes(&conn, "other", "old", "2026-07-10 21:13:00");
        assert!(list_facts_with_view(&conn, "n", 10, 0, &StateView::Current).is_err());
    }
}
