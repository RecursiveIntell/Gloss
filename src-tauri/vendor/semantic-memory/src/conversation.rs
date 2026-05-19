//! Session and message CRUD for conversation storage.

#[cfg(feature = "hnsw")]
use crate::db::{enqueue_pending_index_op, PendingIndexOpKind};
use crate::db::{parse_optional_json, parse_role, with_transaction};
use crate::error::MemoryError;
use crate::quantize::{self, Quantizer};
use crate::search;
use crate::types::{Message, Role, SearchResult, SearchSourceType, Session};
use crate::{as_str_slice, merge_trace_ctx, to_owned_string_vec, MemoryStore};
use rusqlite::{params, Connection};
use stack_ids::TraceCtx;

/// Create a new conversation session and return its UUID.
pub fn create_session(
    conn: &Connection,
    channel: &str,
    metadata: Option<&serde_json::Value>,
) -> Result<String, MemoryError> {
    let id = uuid::Uuid::new_v4().to_string();
    let metadata_str = metadata.map(|m| m.to_string());
    conn.execute(
        "INSERT INTO sessions (id, channel, metadata) VALUES (?1, ?2, ?3)",
        params![id, channel, metadata_str],
    )?;
    Ok(id)
}

/// Append a message to a session without search indexes.
#[allow(dead_code)]
pub fn add_message(
    conn: &Connection,
    session_id: &str,
    role: Role,
    content: &str,
    token_count: Option<u32>,
    metadata: Option<&serde_json::Value>,
) -> Result<i64, MemoryError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1)",
        params![session_id],
        |row| row.get(0),
    )?;
    if !exists {
        return Err(MemoryError::SessionNotFound(session_id.to_string()));
    }

    let metadata_str = metadata.map(|m| m.to_string());
    with_transaction(conn, |tx| {
        tx.execute(
            "INSERT INTO messages (session_id, role, content, token_count, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session_id,
                role.as_str(),
                content,
                token_count,
                metadata_str
            ],
        )?;
        let msg_id = tx.last_insert_rowid();
        tx.execute(
            "UPDATE sessions SET updated_at = datetime('now') WHERE id = ?1",
            params![session_id],
        )?;
        Ok(msg_id)
    })
}

/// Get the most recent N messages from a session in chronological order.
pub fn get_recent_messages(
    conn: &Connection,
    session_id: &str,
    limit: usize,
) -> Result<Vec<Message>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, session_id, role, content, token_count, created_at, metadata
         FROM messages
         WHERE session_id = ?1
         ORDER BY created_at DESC, id DESC
         LIMIT ?2",
    )?;

    let mut messages: Vec<Message> = stmt
        .query_map(params![session_id, limit as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<u32>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(
            |(id, session_id, role_raw, content, token_count, created_at, metadata_raw)| {
                Ok(Message {
                    role: parse_role("messages", &id.to_string(), &role_raw)?,
                    metadata: parse_optional_json(
                        "messages",
                        &id.to_string(),
                        "metadata",
                        metadata_raw.as_deref(),
                    )?,
                    id,
                    session_id,
                    content,
                    token_count,
                    created_at,
                })
            },
        )
        .collect::<Result<Vec<_>, MemoryError>>()?;

    messages.reverse();
    Ok(messages)
}

/// Get messages from a session while staying under the token budget.
pub fn get_messages_within_budget(
    conn: &Connection,
    session_id: &str,
    max_tokens: u32,
) -> Result<Vec<Message>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, session_id, role, content, token_count, created_at, metadata
         FROM messages
         WHERE session_id = ?1
         ORDER BY created_at DESC, id DESC",
    )?;

    let all_messages: Vec<Message> = stmt
        .query_map(params![session_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<u32>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(
            |(id, session_id, role_raw, content, token_count, created_at, metadata_raw)| {
                Ok(Message {
                    role: parse_role("messages", &id.to_string(), &role_raw)?,
                    metadata: parse_optional_json(
                        "messages",
                        &id.to_string(),
                        "metadata",
                        metadata_raw.as_deref(),
                    )?,
                    id,
                    session_id,
                    content,
                    token_count,
                    created_at,
                })
            },
        )
        .collect::<Result<Vec<_>, MemoryError>>()?;

    let mut collected = Vec::new();
    let mut total_tokens = 0u32;
    for msg in all_messages {
        let msg_tokens = msg
            .token_count
            .unwrap_or_else(|| (msg.content.len() / 4).max(1) as u32);
        let next_total = total_tokens.saturating_add(msg_tokens);
        if next_total > max_tokens && !collected.is_empty() {
            break;
        }
        total_tokens = next_total;
        collected.push(msg);
    }

    collected.reverse();
    Ok(collected)
}

/// Get the total token count for a session.
pub fn session_token_count(conn: &Connection, session_id: &str) -> Result<u64, MemoryError> {
    let count: i64 = conn.query_row(
        "SELECT COALESCE(SUM(token_count), 0) FROM messages WHERE session_id = ?1",
        params![session_id],
        |row| row.get(0),
    )?;
    if count < 0 {
        return Err(MemoryError::CorruptData {
            table: "messages",
            row_id: session_id.to_string(),
            detail: format!("negative token_count aggregate: {count}"),
        });
    }
    Ok(count as u64)
}

/// Append a message with embedding + q8 + FTS entries.
#[allow(clippy::too_many_arguments)]
pub fn add_message_with_embedding_q8(
    conn: &Connection,
    session_id: &str,
    role: Role,
    content: &str,
    token_count: Option<u32>,
    metadata: Option<&serde_json::Value>,
    embedding_bytes: &[u8],
    q8_bytes: Option<&[u8]>,
) -> Result<i64, MemoryError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1)",
        params![session_id],
        |row| row.get(0),
    )?;
    if !exists {
        return Err(MemoryError::SessionNotFound(session_id.to_string()));
    }

    let metadata_str = metadata.map(|m| m.to_string());
    with_transaction(conn, |tx| {
        tx.execute(
            "INSERT INTO messages (session_id, role, content, token_count, metadata, embedding, embedding_q8)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                session_id,
                role.as_str(),
                content,
                token_count,
                metadata_str,
                embedding_bytes,
                q8_bytes
            ],
        )?;
        let msg_id = tx.last_insert_rowid();

        tx.execute(
            "INSERT INTO messages_rowid_map (message_id) VALUES (?1)",
            params![msg_id],
        )?;
        let fts_rowid = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO messages_fts(rowid, content) VALUES (?1, ?2)",
            params![fts_rowid, content],
        )?;

        #[cfg(feature = "hnsw")]
        enqueue_pending_index_op(
            tx,
            &format!("msg:{}", msg_id),
            "message",
            PendingIndexOpKind::Upsert,
        )?;
        crate::db::invalidate_derived_vector_artifact(tx, &format!("msg:{msg_id}"))?;

        tx.execute(
            "UPDATE sessions SET updated_at = datetime('now') WHERE id = ?1",
            params![session_id],
        )?;

        Ok(msg_id)
    })
}

/// Backward-compatible wrapper for embedded messages without q8 input.
#[allow(dead_code, clippy::too_many_arguments)]
pub fn add_message_with_embedding(
    conn: &Connection,
    session_id: &str,
    role: Role,
    content: &str,
    token_count: Option<u32>,
    metadata: Option<&serde_json::Value>,
    embedding_bytes: &[u8],
) -> Result<i64, MemoryError> {
    add_message_with_embedding_q8(
        conn,
        session_id,
        role,
        content,
        token_count,
        metadata,
        embedding_bytes,
        None,
    )
}

/// Append a message with FTS indexing but no embedding.
pub fn add_message_with_fts(
    conn: &Connection,
    session_id: &str,
    role: Role,
    content: &str,
    token_count: Option<u32>,
    metadata: Option<&serde_json::Value>,
) -> Result<i64, MemoryError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1)",
        params![session_id],
        |row| row.get(0),
    )?;
    if !exists {
        return Err(MemoryError::SessionNotFound(session_id.to_string()));
    }

    let metadata_str = metadata.map(|m| m.to_string());
    with_transaction(conn, |tx| {
        tx.execute(
            "INSERT INTO messages (session_id, role, content, token_count, metadata, embedding, embedding_q8)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL)",
            params![session_id, role.as_str(), content, token_count, metadata_str],
        )?;
        let msg_id = tx.last_insert_rowid();

        tx.execute(
            "INSERT INTO messages_rowid_map (message_id) VALUES (?1)",
            params![msg_id],
        )?;
        let fts_rowid = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO messages_fts(rowid, content) VALUES (?1, ?2)",
            params![fts_rowid, content],
        )?;
        tx.execute(
            "UPDATE sessions SET updated_at = datetime('now') WHERE id = ?1",
            params![session_id],
        )?;

        Ok(msg_id)
    })
}

/// Delete a session and all its messages.
pub fn delete_session(conn: &Connection, session_id: &str) -> Result<(), MemoryError> {
    with_transaction(conn, |tx| {
        let fts_data: Vec<(i64, String, i64, bool)> = {
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

        for (msg_id, content, fts_rowid, has_embedding) in &fts_data {
            tx.execute(
                "INSERT INTO messages_fts(messages_fts, rowid, content) VALUES('delete', ?1, ?2)",
                params![fts_rowid, content],
            )?;

            #[cfg(feature = "hnsw")]
            if *has_embedding {
                enqueue_pending_index_op(
                    tx,
                    &format!("msg:{}", msg_id),
                    "message",
                    PendingIndexOpKind::Delete,
                )?;
            }

            #[cfg(not(feature = "hnsw"))]
            {
                let _ = msg_id;
                let _ = has_embedding;
            }
            if *has_embedding {
                crate::db::invalidate_derived_vector_artifact(tx, &format!("msg:{msg_id}"))?;
            }
        }

        let affected = tx.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
        if affected == 0 {
            return Err(MemoryError::SessionNotFound(session_id.to_string()));
        }

        Ok(())
    })
}

/// List recent sessions with message counts.
pub fn list_sessions(
    conn: &Connection,
    limit: usize,
    offset: usize,
) -> Result<Vec<Session>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.channel, s.created_at, s.updated_at, s.metadata,
                COUNT(m.id) AS message_count
         FROM sessions s
         LEFT JOIN messages m ON m.session_id = s.id
         GROUP BY s.id
         ORDER BY s.updated_at DESC
         LIMIT ?1 OFFSET ?2",
    )?;

    let sessions = stmt
        .query_map(params![limit as i64, offset as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)? as u32,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(
            |(id, channel, created_at, updated_at, metadata_raw, message_count)| {
                Ok(Session {
                    metadata: parse_optional_json(
                        "sessions",
                        &id,
                        "metadata",
                        metadata_raw.as_deref(),
                    )?,
                    id,
                    channel,
                    created_at,
                    updated_at,
                    message_count,
                })
            },
        )
        .collect::<Result<Vec<_>, MemoryError>>()?;

    Ok(sessions)
}

/// Update a session channel.
pub fn rename_session(
    conn: &Connection,
    session_id: &str,
    new_channel: &str,
) -> Result<(), MemoryError> {
    let affected = conn.execute(
        "UPDATE sessions SET channel = ?1, updated_at = datetime('now') WHERE id = ?2",
        params![new_channel, session_id],
    )?;
    if affected == 0 {
        return Err(MemoryError::SessionNotFound(session_id.to_string()));
    }
    Ok(())
}

impl MemoryStore {
    /// Create a new conversation session. Returns the session ID (UUID v4).
    pub async fn create_session(&self, channel: &str) -> Result<String, MemoryError> {
        let channel = channel.to_string();
        self.with_write_conn(move |conn| create_session(conn, &channel, None))
            .await
    }

    /// Create a new conversation session with metadata.
    ///
    /// Metadata can be used to carry namespace tags and trace data for retention
    /// and deletion policy decisions.
    pub async fn create_session_with_metadata(
        &self,
        channel: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<String, MemoryError> {
        let channel = channel.to_string();
        self.with_write_conn(move |conn| create_session(conn, &channel, metadata.as_ref()))
            .await
    }

    /// Rename a session's channel (display name).
    pub async fn rename_session(
        &self,
        session_id: &str,
        new_channel: &str,
    ) -> Result<(), MemoryError> {
        let sid = session_id.to_string();
        let ch = new_channel.to_string();
        self.with_write_conn(move |conn| rename_session(conn, &sid, &ch))
            .await
    }

    /// List recent sessions, newest first.
    pub async fn list_sessions(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Session>, MemoryError> {
        self.with_read_conn(move |conn| list_sessions(conn, limit, offset))
            .await
    }

    /// Delete a session and all its messages.
    ///
    /// Cleans up HNSW entries for embedded messages before CASCADE delete.
    pub async fn delete_session(&self, session_id: &str) -> Result<(), MemoryError> {
        let sid = session_id.to_string();
        self.with_write_conn(move |conn| delete_session(conn, &sid))
            .await?;

        #[cfg(feature = "hnsw")]
        self.sync_pending_hnsw_ops_best_effort("delete_session")
            .await;

        Ok(())
    }

    /// Append a message to a session. Returns the message's auto-increment ID.
    pub async fn add_message(
        &self,
        session_id: &str,
        role: Role,
        content: &str,
        token_count: Option<u32>,
        metadata: Option<serde_json::Value>,
    ) -> Result<i64, MemoryError> {
        self.add_message_with_trace(session_id, role, content, token_count, metadata, None)
            .await
    }

    /// Append a message to a session with optional trace metadata.
    pub async fn add_message_with_trace(
        &self,
        session_id: &str,
        role: Role,
        content: &str,
        token_count: Option<u32>,
        metadata: Option<serde_json::Value>,
        trace_ctx: Option<&TraceCtx>,
    ) -> Result<i64, MemoryError> {
        self.add_message_embedded_with_trace(
            session_id,
            role,
            content,
            token_count,
            metadata,
            trace_ctx,
        )
        .await
    }

    /// Append a message to a session with FTS indexing but no embedding.
    ///
    /// Fallback path when embedding fails: messages still appear in conversation
    /// history and are findable via BM25 search, just not via vector search.
    pub async fn add_message_fts(
        &self,
        session_id: &str,
        role: Role,
        content: &str,
        token_count: Option<u32>,
        metadata: Option<serde_json::Value>,
    ) -> Result<i64, MemoryError> {
        self.add_message_fts_with_trace(session_id, role, content, token_count, metadata, None)
            .await
    }

    /// Append a message with FTS indexing and optional trace metadata.
    pub async fn add_message_fts_with_trace(
        &self,
        session_id: &str,
        role: Role,
        content: &str,
        token_count: Option<u32>,
        metadata: Option<serde_json::Value>,
        trace_ctx: Option<&TraceCtx>,
    ) -> Result<i64, MemoryError> {
        self.validate_content("message.content", content)?;

        let effective_token_count =
            token_count.or_else(|| Some(self.inner.token_counter.count_tokens(content) as u32));
        let sid = session_id.to_string();
        let ct = content.to_string();
        let meta = merge_trace_ctx(metadata, trace_ctx);
        self.with_write_conn(move |conn| {
            add_message_with_fts(conn, &sid, role, &ct, effective_token_count, meta.as_ref())
        })
        .await
    }

    /// Get the most recent N messages from a session, in chronological order.
    pub async fn get_recent_messages(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<Message>, MemoryError> {
        let sid = session_id.to_string();
        self.with_read_conn(move |conn| get_recent_messages(conn, &sid, limit))
            .await
    }

    /// Get messages from a session up to `max_tokens` total.
    pub async fn get_messages_within_budget(
        &self,
        session_id: &str,
        max_tokens: u32,
    ) -> Result<Vec<Message>, MemoryError> {
        let sid = session_id.to_string();
        self.with_read_conn(move |conn| get_messages_within_budget(conn, &sid, max_tokens))
            .await
    }

    /// Get total token count for a session.
    pub async fn session_token_count(&self, session_id: &str) -> Result<u64, MemoryError> {
        let sid = session_id.to_string();
        self.with_read_conn(move |conn| session_token_count(conn, &sid))
            .await
    }

    /// Append a message to a session with automatic embedding and FTS indexing.
    pub async fn add_message_embedded(
        &self,
        session_id: &str,
        role: Role,
        content: &str,
        token_count: Option<u32>,
        metadata: Option<serde_json::Value>,
    ) -> Result<i64, MemoryError> {
        self.add_message_embedded_with_trace(session_id, role, content, token_count, metadata, None)
            .await
    }

    /// Append an embedded message with optional trace metadata.
    pub async fn add_message_embedded_with_trace(
        &self,
        session_id: &str,
        role: Role,
        content: &str,
        token_count: Option<u32>,
        metadata: Option<serde_json::Value>,
        trace_ctx: Option<&TraceCtx>,
    ) -> Result<i64, MemoryError> {
        self.validate_content("message.content", content)?;

        let effective_token_count =
            token_count.or_else(|| Some(self.inner.token_counter.count_tokens(content) as u32));

        let embedding = self.embed_text_internal(content).await?;
        self.validate_embedding_dimensions(&embedding)?;
        let embedding_bytes = crate::db::embedding_to_bytes(&embedding);
        // INTENTIONAL: q8 quantization is an optional search optimization; missing q8 is non-fatal
        let q8_bytes = Quantizer::new(self.inner.config.embedding.dimensions)
            .quantize(&embedding)
            .map(|qv| quantize::pack_quantized(&qv))
            .ok();

        let sid = session_id.to_string();
        let ct = content.to_string();
        let meta = merge_trace_ctx(metadata, trace_ctx);
        let msg_id = self
            .with_write_conn(move |conn| {
                add_message_with_embedding_q8(
                    conn,
                    &sid,
                    role,
                    &ct,
                    effective_token_count,
                    meta.as_ref(),
                    &embedding_bytes,
                    q8_bytes.as_deref(),
                )
            })
            .await?;

        #[cfg(feature = "hnsw")]
        self.sync_pending_hnsw_ops_best_effort("add_message_embedded")
            .await;

        Ok(msg_id)
    }

    /// Hybrid search over conversation messages only.
    pub async fn search_conversations(
        &self,
        query: &str,
        top_k: Option<usize>,
        session_ids: Option<&[&str]>,
    ) -> Result<Vec<SearchResult>, MemoryError> {
        const MAX_TOP_K: usize = 1_000;
        let k = top_k
            .unwrap_or(self.inner.config.search.default_top_k)
            .min(MAX_TOP_K);

        let query_embedding = self.embed_text_internal(query).await?;

        #[cfg(feature = "hnsw")]
        let hnsw_hits = {
            let index = self
                .inner
                .hnsw_index
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            let candidates = self
                .inner
                .config
                .search
                .candidate_pool_size
                .max(k.saturating_mul(3))
                .min(MAX_TOP_K.saturating_mul(10));
            let query_embedding_for_hnsw = query_embedding.clone();
            match tokio::task::spawn_blocking(move || {
                index.search(&query_embedding_for_hnsw, candidates)
            })
            .await
            {
                Ok(Ok(hits)) => hits,
                Ok(Err(err)) => {
                    tracing::error!(
                        "HNSW conversation search failed, falling back to brute-force message search: {}",
                        err
                    );
                    Vec::new()
                }
                Err(err) => {
                    tracing::error!(
                        "HNSW conversation search task failed, falling back to brute-force message search: {}",
                        err
                    );
                    Vec::new()
                }
            }
        };

        let q = query.to_string();
        let config = self.inner.config.search.clone();
        let sids_owned = to_owned_string_vec(session_ids);

        #[cfg(feature = "hnsw")]
        let hnsw_hits_owned = hnsw_hits;

        self.with_read_conn(move |conn| {
            let sids_refs = as_str_slice(&sids_owned);
            let sids_slice: Option<&[&str]> = sids_refs.as_deref();
            #[cfg(feature = "hnsw")]
            {
                if hnsw_hits_owned.is_empty() {
                    search::hybrid_search(
                        conn,
                        &q,
                        &query_embedding,
                        &config,
                        k,
                        None,
                        Some(&[SearchSourceType::Messages]),
                        sids_slice,
                    )
                } else {
                    search::hybrid_search_with_hnsw(
                        conn,
                        &q,
                        &query_embedding,
                        &config,
                        k,
                        None,
                        Some(&[SearchSourceType::Messages]),
                        sids_slice,
                        &hnsw_hits_owned,
                    )
                }
            }
            #[cfg(not(feature = "hnsw"))]
            {
                search::hybrid_search(
                    conn,
                    &q,
                    &query_embedding,
                    &config,
                    k,
                    None,
                    Some(&[SearchSourceType::Messages]),
                    sids_slice,
                )
            }
        })
        .await
    }
}
