use crate::db::migrations;
use crate::error::GlossError;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Per-notebook database handle (notebook.db).
pub struct NotebookDb {
    pub conn: Connection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub source_type: String,
    pub title: String,
    pub original_filename: Option<String>,
    pub file_hash: Option<String>,
    pub url: Option<String>,
    pub file_path: Option<String>,
    pub content_text: Option<String>,
    pub word_count: Option<i32>,
    pub metadata: Option<String>,
    pub summary: Option<String>,
    pub summary_model: Option<String>,
    pub status: String,
    pub error_message: Option<String>,
    pub selected: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub id: String,
    pub source_id: String,
    pub chunk_index: i32,
    pub content: String,
    pub token_count: Option<i32>,
    pub start_offset: Option<i32>,
    pub end_offset: Option<i32>,
    pub metadata: Option<String>,
    pub embedding_id: Option<i64>,
    pub embedding_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: Option<String>,
    pub style: String,
    pub custom_goal: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub role: String,
    pub content: String,
    pub citations: Option<String>,
    pub model_used: Option<String>,
    pub tokens_prompt: Option<i32>,
    pub tokens_response: Option<i32>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub title: Option<String>,
    pub content: String,
    pub note_type: String,
    pub citations: Option<String>,
    pub pinned: bool,
    pub source_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotebookStats {
    pub source_count: u32,
    pub ready_count: u32,
    pub error_count: u32,
    pub missing_summaries: u32,
    pub chunk_count: u32,
    pub sources_with_chunks: u32,
    pub total_words: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudioOutput {
    pub id: String,
    pub output_type: String,
    pub title: Option<String>,
    pub prompt_used: String,
    pub raw_content: Option<String>,
    pub config: Option<String>,
    pub source_ids: Option<String>,
    pub file_path: Option<String>,
    pub status: String,
    pub error_message: Option<String>,
    pub created_at: String,
}

impl NotebookDb {
    /// Open (or create) a per-notebook database.
    pub fn open(path: &Path) -> Result<Self, GlossError> {
        let conn = Connection::open(path)?;
        migrations::migrate_notebook_db(&conn)?;
        Ok(Self { conn })
    }

    // -- Sources --

    /// List all sources (without content_text — use get_source for full content).
    pub fn list_sources(&self) -> Result<Vec<Source>, GlossError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source_type, title, original_filename, file_hash, url, file_path,
                    word_count, metadata, summary, summary_model,
                    status, error_message, selected, created_at, updated_at
             FROM sources ORDER BY title ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Source {
                id: row.get(0)?,
                source_type: row.get(1)?,
                title: row.get(2)?,
                original_filename: row.get(3)?,
                file_hash: row.get(4)?,
                url: row.get(5)?,
                file_path: row.get(6)?,
                content_text: None, // Excluded — too large for listing
                word_count: row.get(7)?,
                metadata: row.get(8)?,
                summary: row.get(9)?,
                summary_model: row.get(10)?,
                status: row.get(11)?,
                error_message: row.get(12)?,
                selected: row.get(13)?,
                created_at: row.get(14)?,
                updated_at: row.get(15)?,
            })
        })?;
        let mut sources = Vec::new();
        for row in rows {
            sources.push(row?);
        }
        Ok(sources)
    }

    /// Insert a new source.
    pub fn insert_source(&self, source: &Source) -> Result<(), GlossError> {
        self.conn.execute(
            "INSERT INTO sources (id, source_type, title, original_filename, file_hash, url,
                                  file_path, content_text, word_count, metadata, summary,
                                  summary_model, status, error_message, selected)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            rusqlite::params![
                source.id,
                source.source_type,
                source.title,
                source.original_filename,
                source.file_hash,
                source.url,
                source.file_path,
                source.content_text,
                source.word_count,
                source.metadata,
                source.summary,
                source.summary_model,
                source.status,
                source.error_message,
                source.selected,
            ],
        )?;
        Ok(())
    }

    /// Update source status.
    pub fn update_source_status(
        &self,
        source_id: &str,
        status: &str,
        error_message: Option<&str>,
    ) -> Result<(), GlossError> {
        self.conn.execute(
            "UPDATE sources SET status = ?1, error_message = ?2, updated_at = datetime('now')
             WHERE id = ?3",
            rusqlite::params![status, error_message, source_id],
        )?;
        Ok(())
    }

    /// Update source content after extraction.
    pub fn update_source_content(
        &self,
        source_id: &str,
        content_text: &str,
        word_count: i32,
    ) -> Result<(), GlossError> {
        self.conn.execute(
            "UPDATE sources SET content_text = ?1, word_count = ?2, updated_at = datetime('now')
             WHERE id = ?3",
            rusqlite::params![content_text, word_count, source_id],
        )?;
        Ok(())
    }

    /// Update source summary.
    pub fn update_source_summary(
        &self,
        source_id: &str,
        summary: &str,
        model: &str,
    ) -> Result<(), GlossError> {
        self.conn.execute(
            "UPDATE sources SET summary = ?1, summary_model = ?2, updated_at = datetime('now')
             WHERE id = ?3",
            rusqlite::params![summary, model, source_id],
        )?;
        Ok(())
    }

    /// Get a source by ID.
    pub fn get_source(&self, source_id: &str) -> Result<Source, GlossError> {
        self.conn
            .query_row(
                "SELECT id, source_type, title, original_filename, file_hash, url, file_path,
                        content_text, word_count, metadata, summary, summary_model,
                        status, error_message, selected, created_at, updated_at
                 FROM sources WHERE id = ?1",
                [source_id],
                |row| {
                    Ok(Source {
                        id: row.get(0)?,
                        source_type: row.get(1)?,
                        title: row.get(2)?,
                        original_filename: row.get(3)?,
                        file_hash: row.get(4)?,
                        url: row.get(5)?,
                        file_path: row.get(6)?,
                        content_text: row.get(7)?,
                        word_count: row.get(8)?,
                        metadata: row.get(9)?,
                        summary: row.get(10)?,
                        summary_model: row.get(11)?,
                        status: row.get(12)?,
                        error_message: row.get(13)?,
                        selected: row.get(14)?,
                        created_at: row.get(15)?,
                        updated_at: row.get(16)?,
                    })
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    GlossError::NotFound(format!("Source {source_id} not found"))
                }
                other => GlossError::Database(other),
            })
    }

    /// Delete a source and its chunks (cascade).
    pub fn delete_source(&self, source_id: &str) -> Result<(), GlossError> {
        self.conn
            .execute("DELETE FROM sources WHERE id = ?1", [source_id])?;
        Ok(())
    }

    /// Get count of sources.
    pub fn source_count(&self) -> Result<i32, GlossError> {
        let count: i32 = self
            .conn
            .query_row("SELECT COUNT(*) FROM sources", [], |row| row.get(0))?;
        Ok(count)
    }

    // -- Chunks --

    /// Insert a chunk.
    pub fn insert_chunk(&self, chunk: &Chunk) -> Result<i64, GlossError> {
        self.conn.execute(
            "INSERT INTO chunks (id, source_id, chunk_index, content, token_count,
                                 start_offset, end_offset, metadata, embedding_id, embedding_model)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                chunk.id,
                chunk.source_id,
                chunk.chunk_index,
                chunk.content,
                chunk.token_count,
                chunk.start_offset,
                chunk.end_offset,
                chunk.metadata,
                chunk.embedding_id,
                chunk.embedding_model,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Update chunk embedding_id after HNSW insertion.
    pub fn update_chunk_embedding(
        &self,
        chunk_id: &str,
        embedding_id: i64,
        model: &str,
    ) -> Result<(), GlossError> {
        self.conn.execute(
            "UPDATE chunks SET embedding_id = ?1, embedding_model = ?2 WHERE id = ?3",
            rusqlite::params![embedding_id, model, chunk_id],
        )?;
        Ok(())
    }

    /// Get chunks for a source.
    pub fn get_chunks_for_source(&self, source_id: &str) -> Result<Vec<Chunk>, GlossError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source_id, chunk_index, content, token_count, start_offset, end_offset,
                    metadata, embedding_id, embedding_model
             FROM chunks WHERE source_id = ?1 ORDER BY chunk_index",
        )?;
        let rows = stmt.query_map([source_id], |row| {
            Ok(Chunk {
                id: row.get(0)?,
                source_id: row.get(1)?,
                chunk_index: row.get(2)?,
                content: row.get(3)?,
                token_count: row.get(4)?,
                start_offset: row.get(5)?,
                end_offset: row.get(6)?,
                metadata: row.get(7)?,
                embedding_id: row.get(8)?,
                embedding_model: row.get(9)?,
            })
        })?;
        let mut chunks = Vec::new();
        for row in rows {
            chunks.push(row?);
        }
        Ok(chunks)
    }

    /// Get a chunk by ID.
    pub fn get_chunk(&self, chunk_id: &str) -> Result<Chunk, GlossError> {
        self.conn
            .query_row(
                "SELECT id, source_id, chunk_index, content, token_count, start_offset, end_offset,
                        metadata, embedding_id, embedding_model
                 FROM chunks WHERE id = ?1",
                [chunk_id],
                |row| {
                    Ok(Chunk {
                        id: row.get(0)?,
                        source_id: row.get(1)?,
                        chunk_index: row.get(2)?,
                        content: row.get(3)?,
                        token_count: row.get(4)?,
                        start_offset: row.get(5)?,
                        end_offset: row.get(6)?,
                        metadata: row.get(7)?,
                        embedding_id: row.get(8)?,
                        embedding_model: row.get(9)?,
                    })
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    GlossError::NotFound(format!("Chunk {chunk_id} not found"))
                }
                other => GlossError::Database(other),
            })
    }

    /// Get chunks by their IDs.
    pub fn get_chunks_by_ids(&self, chunk_ids: &[String]) -> Result<Vec<Chunk>, GlossError> {
        if chunk_ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: Vec<String> = (0..chunk_ids.len()).map(|i| format!("?{}", i + 1)).collect();
        let sql = format!(
            "SELECT id, source_id, chunk_index, content, token_count, start_offset, end_offset,
                    metadata, embedding_id, embedding_model
             FROM chunks WHERE id IN ({})",
            placeholders.join(", ")
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = chunk_ids
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok(Chunk {
                id: row.get(0)?,
                source_id: row.get(1)?,
                chunk_index: row.get(2)?,
                content: row.get(3)?,
                token_count: row.get(4)?,
                start_offset: row.get(5)?,
                end_offset: row.get(6)?,
                metadata: row.get(7)?,
                embedding_id: row.get(8)?,
                embedding_model: row.get(9)?,
            })
        })?;
        let mut chunks = Vec::new();
        for row in rows {
            chunks.push(row?);
        }
        Ok(chunks)
    }

    /// Get chunk by embedding_id (HNSW label).
    pub fn get_chunk_by_embedding_id(&self, embedding_id: i64) -> Result<Chunk, GlossError> {
        self.conn
            .query_row(
                "SELECT id, source_id, chunk_index, content, token_count, start_offset, end_offset,
                        metadata, embedding_id, embedding_model
                 FROM chunks WHERE embedding_id = ?1",
                [embedding_id],
                |row| {
                    Ok(Chunk {
                        id: row.get(0)?,
                        source_id: row.get(1)?,
                        chunk_index: row.get(2)?,
                        content: row.get(3)?,
                        token_count: row.get(4)?,
                        start_offset: row.get(5)?,
                        end_offset: row.get(6)?,
                        metadata: row.get(7)?,
                        embedding_id: row.get(8)?,
                        embedding_model: row.get(9)?,
                    })
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    GlossError::NotFound(format!("Chunk with embedding_id {embedding_id} not found"))
                }
                other => GlossError::Database(other),
            })
    }

    /// FTS5 search for chunks.
    pub fn fts_search(&self, query: &str, limit: usize) -> Result<Vec<(i64, f64)>, GlossError> {
        let mut stmt = self.conn.prepare(
            "SELECT rowid, rank FROM chunks_fts WHERE chunks_fts MATCH ?1
             ORDER BY rank LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![query, limit as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get chunk by rowid (for FTS results).
    pub fn get_chunk_by_rowid(&self, rowid: i64) -> Result<Chunk, GlossError> {
        self.conn
            .query_row(
                "SELECT id, source_id, chunk_index, content, token_count, start_offset, end_offset,
                        metadata, embedding_id, embedding_model
                 FROM chunks WHERE rowid = ?1",
                [rowid],
                |row| {
                    Ok(Chunk {
                        id: row.get(0)?,
                        source_id: row.get(1)?,
                        chunk_index: row.get(2)?,
                        content: row.get(3)?,
                        token_count: row.get(4)?,
                        start_offset: row.get(5)?,
                        end_offset: row.get(6)?,
                        metadata: row.get(7)?,
                        embedding_id: row.get(8)?,
                        embedding_model: row.get(9)?,
                    })
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    GlossError::NotFound(format!("Chunk with rowid {rowid} not found"))
                }
                other => GlossError::Database(other),
            })
    }

    // -- Conversations --

    /// List all conversations.
    pub fn list_conversations(&self) -> Result<Vec<Conversation>, GlossError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, style, custom_goal, created_at, updated_at
             FROM conversations ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                title: row.get(1)?,
                style: row.get(2)?,
                custom_goal: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?;
        let mut convs = Vec::new();
        for row in rows {
            convs.push(row?);
        }
        Ok(convs)
    }

    /// Create a new conversation.
    pub fn create_conversation(&self, id: &str) -> Result<(), GlossError> {
        self.conn.execute(
            "INSERT INTO conversations (id) VALUES (?1)",
            [id],
        )?;
        Ok(())
    }

    /// Delete a conversation.
    pub fn delete_conversation(&self, id: &str) -> Result<(), GlossError> {
        self.conn
            .execute("DELETE FROM conversations WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Update conversation title.
    pub fn update_conversation_title(&self, id: &str, title: &str) -> Result<(), GlossError> {
        self.conn.execute(
            "UPDATE conversations SET title = ?1, updated_at = datetime('now') WHERE id = ?2",
            rusqlite::params![title, id],
        )?;
        Ok(())
    }

    // -- Messages --

    /// Load messages for a conversation.
    pub fn load_messages(&self, conversation_id: &str) -> Result<Vec<Message>, GlossError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, conversation_id, role, content, citations, model_used,
                    tokens_prompt, tokens_response, created_at
             FROM messages WHERE conversation_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([conversation_id], |row| {
            Ok(Message {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                citations: row.get(4)?,
                model_used: row.get(5)?,
                tokens_prompt: row.get(6)?,
                tokens_response: row.get(7)?,
                created_at: row.get(8)?,
            })
        })?;
        let mut msgs = Vec::new();
        for row in rows {
            msgs.push(row?);
        }
        Ok(msgs)
    }

    /// Insert a message.
    pub fn insert_message(&self, msg: &Message) -> Result<(), GlossError> {
        self.conn.execute(
            "INSERT INTO messages (id, conversation_id, role, content, citations, model_used,
                                   tokens_prompt, tokens_response)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                msg.id,
                msg.conversation_id,
                msg.role,
                msg.content,
                msg.citations,
                msg.model_used,
                msg.tokens_prompt,
                msg.tokens_response,
            ],
        )?;
        // Update conversation's updated_at
        self.conn.execute(
            "UPDATE conversations SET updated_at = datetime('now') WHERE id = ?1",
            [&msg.conversation_id],
        )?;
        Ok(())
    }

    /// Get a message by ID.
    pub fn get_message(&self, message_id: &str) -> Result<Message, GlossError> {
        self.conn
            .query_row(
                "SELECT id, conversation_id, role, content, citations, model_used,
                        tokens_prompt, tokens_response, created_at
                 FROM messages WHERE id = ?1",
                [message_id],
                |row| {
                    Ok(Message {
                        id: row.get(0)?,
                        conversation_id: row.get(1)?,
                        role: row.get(2)?,
                        content: row.get(3)?,
                        citations: row.get(4)?,
                        model_used: row.get(5)?,
                        tokens_prompt: row.get(6)?,
                        tokens_response: row.get(7)?,
                        created_at: row.get(8)?,
                    })
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    GlossError::NotFound(format!("Message {message_id} not found"))
                }
                other => GlossError::Database(other),
            })
    }

    // -- Notes --

    /// List all notes (pinned first, then by date).
    pub fn list_notes(&self) -> Result<Vec<Note>, GlossError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, content, note_type, citations, pinned, source_id,
                    created_at, updated_at
             FROM notes ORDER BY pinned DESC, updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Note {
                id: row.get(0)?,
                title: row.get(1)?,
                content: row.get(2)?,
                note_type: row.get(3)?,
                citations: row.get(4)?,
                pinned: row.get(5)?,
                source_id: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;
        let mut notes = Vec::new();
        for row in rows {
            notes.push(row?);
        }
        Ok(notes)
    }

    /// Create a note.
    pub fn create_note(&self, note: &Note) -> Result<(), GlossError> {
        self.conn.execute(
            "INSERT INTO notes (id, title, content, note_type, citations, pinned, source_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                note.id,
                note.title,
                note.content,
                note.note_type,
                note.citations,
                note.pinned,
                note.source_id,
            ],
        )?;
        Ok(())
    }

    /// Update a note.
    pub fn update_note(
        &self,
        note_id: &str,
        title: Option<&str>,
        content: Option<&str>,
    ) -> Result<(), GlossError> {
        if let Some(t) = title {
            self.conn.execute(
                "UPDATE notes SET title = ?1, updated_at = datetime('now') WHERE id = ?2",
                rusqlite::params![t, note_id],
            )?;
        }
        if let Some(c) = content {
            self.conn.execute(
                "UPDATE notes SET content = ?1, updated_at = datetime('now') WHERE id = ?2",
                rusqlite::params![c, note_id],
            )?;
        }
        Ok(())
    }

    /// Toggle pin on a note.
    pub fn toggle_pin(&self, note_id: &str) -> Result<(), GlossError> {
        self.conn.execute(
            "UPDATE notes SET pinned = NOT pinned, updated_at = datetime('now') WHERE id = ?1",
            [note_id],
        )?;
        Ok(())
    }

    /// Delete a note.
    pub fn delete_note(&self, note_id: &str) -> Result<(), GlossError> {
        self.conn
            .execute("DELETE FROM notes WHERE id = ?1", [note_id])?;
        Ok(())
    }

    // -- Notebook Config --

    /// Get a notebook config value.
    pub fn get_config(&self, key: &str) -> Result<Option<String>, GlossError> {
        let result = self.conn.query_row(
            "SELECT value FROM notebook_config WHERE key = ?1",
            [key],
            |row| row.get(0),
        );
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(GlossError::Database(e)),
        }
    }

    /// Set a notebook config value.
    pub fn set_config(&self, key: &str, value: &str) -> Result<(), GlossError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO notebook_config (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    /// Get all selected source IDs.
    pub fn get_selected_source_ids(&self) -> Result<Vec<String>, GlossError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM sources WHERE selected = 1")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    /// List sources that need summaries (ready but no summary).
    pub fn list_sources_needing_summary(&self) -> Result<Vec<Source>, GlossError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source_type, title, original_filename, file_hash, url, file_path,
                    content_text, word_count, metadata, summary, summary_model,
                    status, error_message, selected, created_at, updated_at
             FROM sources WHERE status = 'ready' AND summary IS NULL
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Source {
                id: row.get(0)?,
                source_type: row.get(1)?,
                title: row.get(2)?,
                original_filename: row.get(3)?,
                file_hash: row.get(4)?,
                url: row.get(5)?,
                file_path: row.get(6)?,
                content_text: row.get(7)?,
                word_count: row.get(8)?,
                metadata: row.get(9)?,
                summary: row.get(10)?,
                summary_model: row.get(11)?,
                status: row.get(12)?,
                error_message: row.get(13)?,
                selected: row.get(14)?,
                created_at: row.get(15)?,
                updated_at: row.get(16)?,
            })
        })?;
        let mut sources = Vec::new();
        for row in rows {
            sources.push(row?);
        }
        Ok(sources)
    }

    /// Get all embedding IDs for a source's chunks (for HNSW cleanup before deletion).
    pub fn get_embedding_ids_for_source(&self, source_id: &str) -> Result<Vec<u64>, GlossError> {
        let mut stmt = self.conn.prepare(
            "SELECT embedding_id FROM chunks WHERE source_id = ? AND embedding_id IS NOT NULL",
        )?;
        let ids = stmt
            .query_map(rusqlite::params![source_id], |row| row.get::<_, i64>(0))?
            .filter_map(|r| r.ok())
            .map(|id| id as u64)
            .collect();
        Ok(ids)
    }

    /// Delete all chunks for a source.
    pub fn delete_chunks_for_source(&self, source_id: &str) -> Result<(), GlossError> {
        self.conn
            .execute("DELETE FROM chunks WHERE source_id = ?1", [source_id])?;
        Ok(())
    }

    /// Get notebook-level statistics.
    pub fn get_stats(&self) -> Result<NotebookStats, GlossError> {
        let stats = self.conn.query_row(
            "SELECT
                (SELECT COUNT(*) FROM sources) as source_count,
                (SELECT COUNT(*) FROM sources WHERE status = 'ready') as ready_count,
                (SELECT COUNT(*) FROM sources WHERE status = 'error') as error_count,
                (SELECT COUNT(*) FROM sources WHERE summary IS NULL AND status = 'ready') as missing_summaries,
                (SELECT COUNT(*) FROM chunks) as chunk_count,
                (SELECT COUNT(DISTINCT source_id) FROM chunks) as sources_with_chunks,
                (SELECT COALESCE(SUM(word_count), 0) FROM sources) as total_words",
            [],
            |row| {
                Ok(NotebookStats {
                    source_count: row.get(0)?,
                    ready_count: row.get(1)?,
                    error_count: row.get(2)?,
                    missing_summaries: row.get(3)?,
                    chunk_count: row.get(4)?,
                    sources_with_chunks: row.get(5)?,
                    total_words: row.get(6)?,
                })
            },
        )?;
        Ok(stats)
    }

    /// Get all summaries for selected sources.
    pub fn get_selected_summaries(&self) -> Result<Vec<(String, String, Option<String>)>, GlossError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, summary FROM sources WHERE selected = 1 AND summary IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        let mut summaries = Vec::new();
        for row in rows {
            summaries.push(row?);
        }
        Ok(summaries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_db() -> NotebookDb {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test_notebook.db");
        NotebookDb::open(&path).unwrap()
    }

    #[test]
    fn test_source_crud() {
        let db = test_db();
        let source = Source {
            id: "s1".to_string(),
            source_type: "text".to_string(),
            title: "Test Source".to_string(),
            original_filename: Some("test.txt".to_string()),
            file_hash: None,
            url: None,
            file_path: None,
            content_text: Some("Hello world".to_string()),
            word_count: Some(2),
            metadata: None,
            summary: None,
            summary_model: None,
            status: "ready".to_string(),
            error_message: None,
            selected: true,
            created_at: String::new(),
            updated_at: String::new(),
        };
        db.insert_source(&source).unwrap();
        let sources = db.list_sources().unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].title, "Test Source");

        db.delete_source("s1").unwrap();
        let sources = db.list_sources().unwrap();
        assert_eq!(sources.len(), 0);
    }

    #[test]
    fn test_chunk_and_fts() {
        let db = test_db();
        let source = Source {
            id: "s1".to_string(),
            source_type: "text".to_string(),
            title: "Test".to_string(),
            original_filename: None,
            file_hash: None,
            url: None,
            file_path: None,
            content_text: None,
            word_count: None,
            metadata: None,
            summary: None,
            summary_model: None,
            status: "ready".to_string(),
            error_message: None,
            selected: true,
            created_at: String::new(),
            updated_at: String::new(),
        };
        db.insert_source(&source).unwrap();

        let chunk = Chunk {
            id: "c1".to_string(),
            source_id: "s1".to_string(),
            chunk_index: 0,
            content: "Rust programming language is systems level".to_string(),
            token_count: Some(6),
            start_offset: Some(0),
            end_offset: Some(42),
            metadata: None,
            embedding_id: None,
            embedding_model: None,
        };
        db.insert_chunk(&chunk).unwrap();

        // FTS search
        let results = db.fts_search("rust programming", 10).unwrap();
        assert!(!results.is_empty());
    }

    #[test]
    fn test_conversation_and_messages() {
        let db = test_db();
        db.create_conversation("conv1").unwrap();
        let convs = db.list_conversations().unwrap();
        assert_eq!(convs.len(), 1);

        let msg = Message {
            id: "m1".to_string(),
            conversation_id: "conv1".to_string(),
            role: "user".to_string(),
            content: "Hello".to_string(),
            citations: None,
            model_used: None,
            tokens_prompt: None,
            tokens_response: None,
            created_at: String::new(),
        };
        db.insert_message(&msg).unwrap();
        let msgs = db.load_messages("conv1").unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "Hello");
    }

    #[test]
    fn test_notes_crud() {
        let db = test_db();
        let note = Note {
            id: "n1".to_string(),
            title: Some("My Note".to_string()),
            content: "Some content".to_string(),
            note_type: "manual".to_string(),
            citations: None,
            pinned: false,
            source_id: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        db.create_note(&note).unwrap();
        let notes = db.list_notes().unwrap();
        assert_eq!(notes.len(), 1);

        db.toggle_pin("n1").unwrap();
        let notes = db.list_notes().unwrap();
        assert!(notes[0].pinned);

        db.delete_note("n1").unwrap();
        let notes = db.list_notes().unwrap();
        assert_eq!(notes.len(), 0);
    }

    #[test]
    fn test_cascade_delete_source_removes_chunks() {
        let db = test_db();
        let source = Source {
            id: "s1".to_string(),
            source_type: "text".to_string(),
            title: "Test".to_string(),
            original_filename: None,
            file_hash: None,
            url: None,
            file_path: None,
            content_text: None,
            word_count: None,
            metadata: None,
            summary: None,
            summary_model: None,
            status: "ready".to_string(),
            error_message: None,
            selected: true,
            created_at: String::new(),
            updated_at: String::new(),
        };
        db.insert_source(&source).unwrap();
        let chunk = Chunk {
            id: "c1".to_string(),
            source_id: "s1".to_string(),
            chunk_index: 0,
            content: "Test content".to_string(),
            token_count: None,
            start_offset: None,
            end_offset: None,
            metadata: None,
            embedding_id: None,
            embedding_model: None,
        };
        db.insert_chunk(&chunk).unwrap();

        db.delete_source("s1").unwrap();
        let chunks = db.get_chunks_for_source("s1").unwrap();
        assert_eq!(chunks.len(), 0);
    }
}
