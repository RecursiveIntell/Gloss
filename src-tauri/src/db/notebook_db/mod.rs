use crate::db::migrations;
use crate::error::GlossError;
use crate::memory::types::RetrievalCoverage;
use crate::retrieval::source_scope::ResolvedSourceScope;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Per-notebook database handle (notebook.db).
#[repr(transparent)]
pub struct NotebookDb {
    conn: Connection,
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
    #[serde(default)]
    pub processing_state: Option<SourceProcessingState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceProcessingState {
    pub source_id: String,
    pub lifecycle_status: String,
    pub summary_status: String,
    pub fts_index_status: String,
    pub dense_index_status: String,
    pub semantic_projection_status: String,
    pub last_summary_receipt_id: Option<String>,
    pub last_dense_index_receipt_id: Option<String>,
    pub last_projection_receipt_id: Option<String>,
    pub last_error: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSummaryCandidate {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

pub type ChunkSearchHit = (Chunk, f64);

pub const EMBEDDING_INDEX_SCHEMA_VERSION: i32 = 1;
pub const NATIVE_HNSW_INDEX_ID: &str = "native_hnsw";
pub const SEMANTIC_MEMORY_INDEX_ID: &str = "semantic_memory";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingIndexMetadata {
    pub index_id: String,
    pub provider: String,
    pub model: String,
    pub model_digest: Option<String>,
    pub dimensions: Option<usize>,
    pub schema_version: i32,
    pub status: String,
    pub status_reason: Option<String>,
    pub created_at: Option<String>,
    pub validated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingIndexMetadataStatus {
    Unknown,
    Building,
    Ready,
    Stale,
    Blocked,
}

impl EmbeddingIndexMetadataStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Building => "building",
            Self::Ready => "ready",
            Self::Stale => "stale",
            Self::Blocked => "blocked",
        }
    }
}

impl EmbeddingIndexMetadata {
    pub fn ready(
        index_id: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
        model_digest: Option<String>,
        dimensions: usize,
    ) -> Self {
        Self {
            index_id: index_id.into(),
            provider: provider.into(),
            model: model.into(),
            model_digest,
            dimensions: Some(dimensions),
            schema_version: EMBEDDING_INDEX_SCHEMA_VERSION,
            status: EmbeddingIndexMetadataStatus::Ready.as_str().to_string(),
            status_reason: None,
            created_at: None,
            validated_at: None,
        }
    }

    pub fn identity_matches(&self, expected: &EmbeddingIndexMetadata) -> bool {
        self.derivation_matches(expected)
            && self.status == EmbeddingIndexMetadataStatus::Ready.as_str()
    }

    /// Derivation identity is separate from readiness so an interrupted
    /// Building/Blocked publication can be retried without mixing models.
    pub fn derivation_matches(&self, expected: &EmbeddingIndexMetadata) -> bool {
        self.index_id == expected.index_id
            && self.provider == expected.provider
            && self.model == expected.model
            && self.model_digest == expected.model_digest
            && self.dimensions == expected.dimensions
            && self.schema_version == expected.schema_version
    }
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
pub struct ChatAttemptStatus {
    pub attempt_id: String,
    pub notebook_id: String,
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub user_message_id: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub status: String,
    pub phase: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub request_digest: Option<String>,
    pub response_digest: Option<String>,
    pub partial_policy: Option<String>,
    pub terminal: bool,
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
pub struct SemanticMemoryProjectionStatus {
    pub notebook_id: String,
    pub source_id: String,
    pub status: String,
    pub chunk_count: usize,
    pub projected_chunk_count: usize,
    pub healthy_link_count: usize,
    pub degraded_link_count: usize,
    pub last_receipt_id: Option<String>,
    pub last_error: Option<String>,
    pub artifact_generation_id: Option<String>,
    pub vector_artifact_manifest_digest: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticMemoryProjectionStatusUpdate {
    pub notebook_id: String,
    pub source_id: String,
    pub status: String,
    pub chunk_count: usize,
    pub projected_chunk_count: usize,
    pub healthy_link_count: usize,
    pub degraded_link_count: usize,
    pub last_receipt_id: Option<String>,
    pub last_error: Option<String>,
    pub artifact_generation_id: Option<String>,
    pub vector_artifact_manifest_digest: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SemanticMemoryProjectionSummary {
    pub notebook_id: String,
    pub total_sources: usize,
    pub chunk_bearing_sources: usize,
    pub zero_chunk_sources: usize,
    pub projected_sources: usize,
    pub failed_sources: usize,
    pub skipped_no_chunks: usize,
    pub stale_sources: usize,
    pub partial_sources: usize,
    pub projecting_sources: usize,
    pub healthy_links: usize,
    pub degraded_links: usize,
    pub missing_links: usize,
    pub total_chunks: usize,
    pub projected_chunks: usize,
    pub projection_required: bool,
}

fn projection_status_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SemanticMemoryProjectionStatus> {
    let chunk_count: i64 = row.get(3)?;
    let projected_chunk_count: i64 = row.get(4)?;
    let healthy_link_count: i64 = row.get(5)?;
    let degraded_link_count: i64 = row.get(6)?;
    Ok(SemanticMemoryProjectionStatus {
        notebook_id: row.get(0)?,
        source_id: row.get(1)?,
        status: row.get(2)?,
        chunk_count: chunk_count.max(0) as usize,
        projected_chunk_count: projected_chunk_count.max(0) as usize,
        healthy_link_count: healthy_link_count.max(0) as usize,
        degraded_link_count: degraded_link_count.max(0) as usize,
        last_receipt_id: row.get(7)?,
        last_error: row.get(8)?,
        artifact_generation_id: row.get(9)?,
        vector_artifact_manifest_digest: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn source_processing_state_from_row(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<SourceProcessingState> {
    Ok(SourceProcessingState {
        source_id: row.get(offset)?,
        lifecycle_status: row.get(offset + 1)?,
        summary_status: row.get(offset + 2)?,
        fts_index_status: row.get(offset + 3)?,
        dense_index_status: row.get(offset + 4)?,
        semantic_projection_status: row.get(offset + 5)?,
        last_summary_receipt_id: row.get(offset + 6)?,
        last_dense_index_receipt_id: row.get(offset + 7)?,
        last_projection_receipt_id: row.get(offset + 8)?,
        last_error: row.get(offset + 9)?,
        updated_at: row.get(offset + 10)?,
    })
}

fn studio_output_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StudioOutput> {
    Ok(StudioOutput {
        id: row.get(0)?,
        output_type: row.get(1)?,
        title: row.get(2)?,
        prompt_used: row.get(3)?,
        raw_content: row.get(4)?,
        config: row.get(5)?,
        source_ids: row.get(6)?,
        file_path: row.get(7)?,
        status: row.get(8)?,
        error_message: row.get(9)?,
        prose_content: row.get(10)?,
        created_at: row.get(11)?,
    })
}

#[allow(dead_code)]
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
    pub prose_content: Option<String>,
    pub created_at: String,
}

#[allow(dead_code)]
impl NotebookDb {
    /// Provide read-only access to the underlying connection.
    ///
    /// This is the only sanctioned way for callers outside `db::notebook_db`
    /// to reach the `rusqlite::Connection`.  The field itself is private
    /// to prevent accidental direct mutation or schema changes.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Wrap an already-opened `Connection` as a `NotebookDb` reference.
    ///
    /// Used by the connection pool to vend `NotebookDb` handles from
    /// pooled connections without re-opening the file.
    ///
    /// # Safety
    ///
    /// `NotebookDb` is a single-field newtype around `Connection`.  The two
    /// types have compatible layouts, so casting `&Connection` → `&NotebookDb`
    /// is sound.  The returned reference borrows the input reference and
    /// therefore cannot outlive it.
    pub fn from_conn_ref(conn: &Connection) -> &Self {
        // SAFETY: `NotebookDb` is `#[repr(transparent)]` with a single
        // `Connection` field.  A reference to the field is therefore also a
        // valid reference to the whole struct.  The lifetime is tied to the
        // input reference, so no dangling can occur.
        unsafe { &*(conn as *const Connection as *const NotebookDb) }
    }

    /// Connect to an existing per-notebook database with runtime pragmas.
    pub fn connect(path: &Path) -> Result<Self, GlossError> {
        let conn = Connection::open(path)?;
        migrations::apply_pragmas(&conn)?;
        Ok(Self { conn })
    }

    /// Open (or create) a per-notebook database and run migrations.
    pub fn open(path: &Path) -> Result<Self, GlossError> {
        let db = Self::connect(path)?;
        migrations::migrate_notebook_db(db.conn())?;
        Ok(db)
    }

    // -- Sources --

    /// List all sources (without content_text — use get_source for full content).
    pub fn list_sources(&self) -> Result<Vec<Source>, GlossError> {
        let mut stmt = self.conn.prepare(
            "SELECT sources.id, sources.source_type, sources.title, sources.original_filename,
                    sources.file_hash, sources.url, sources.file_path,
                    sources.word_count, sources.metadata, sources.summary, sources.summary_model,
                    sources.status, sources.error_message, sources.selected,
                    sources.created_at, sources.updated_at,
                    COALESCE(ps.source_id, sources.id),
                    COALESCE(ps.lifecycle_status, sources.status),
                    COALESCE(ps.summary_status, CASE WHEN sources.summary IS NULL OR trim(sources.summary) = '' THEN 'missing' ELSE 'ready' END),
                    COALESCE(ps.fts_index_status, 'missing'),
                    COALESCE(ps.dense_index_status, 'missing'),
                    COALESCE(ps.semantic_projection_status, 'disabled'),
                    ps.last_summary_receipt_id,
                    ps.last_dense_index_receipt_id,
                    ps.last_projection_receipt_id,
                    COALESCE(ps.last_error, sources.error_message),
                    COALESCE(ps.updated_at, sources.updated_at)
             FROM sources
             LEFT JOIN source_processing_state ps ON ps.source_id = sources.id
             ORDER BY sources.title ASC",
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
                processing_state: Some(source_processing_state_from_row(row, 16)?),
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
        self.conn.execute(
            "INSERT OR REPLACE INTO source_processing_state (
                source_id, lifecycle_status, summary_status, fts_index_status,
                dense_index_status, semantic_projection_status, last_error, updated_at
             )
             VALUES (
                ?1, ?2,
                CASE WHEN ?3 IS NULL OR trim(?3) = '' THEN 'missing' ELSE 'ready' END,
                'missing', 'missing', 'disabled', ?4, datetime('now')
             )",
            rusqlite::params![
                source.id,
                source.status,
                source.summary,
                source.error_message,
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
        self.update_source_lifecycle_status(source_id, status, error_message)?;
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

    /// Update source metadata after receipt-bearing enrichment.
    pub fn update_source_metadata(
        &self,
        source_id: &str,
        metadata: Option<&str>,
    ) -> Result<(), GlossError> {
        self.conn.execute(
            "UPDATE sources SET metadata = ?1, updated_at = datetime('now')
             WHERE id = ?2",
            rusqlite::params![metadata, source_id],
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
        self.conn.execute(
            "INSERT INTO source_processing_state (source_id, lifecycle_status, summary_status, updated_at)
             VALUES (?1, COALESCE((SELECT status FROM sources WHERE id = ?1), 'ready'), 'ready', datetime('now'))
             ON CONFLICT(source_id) DO UPDATE SET
                summary_status = 'ready',
                last_error = NULL,
                updated_at = excluded.updated_at",
            [source_id],
        )?;
        Ok(())
    }

    /// Get a source by ID.
    pub fn get_source(&self, source_id: &str) -> Result<Source, GlossError> {
        self.conn
            .query_row(
                "SELECT sources.id, sources.source_type, sources.title, sources.original_filename,
                        sources.file_hash, sources.url, sources.file_path,
                        sources.content_text, sources.word_count, sources.metadata,
                        sources.summary, sources.summary_model,
                        sources.status, sources.error_message, sources.selected,
                        sources.created_at, sources.updated_at,
                        COALESCE(ps.source_id, sources.id),
                        COALESCE(ps.lifecycle_status, sources.status),
                        COALESCE(ps.summary_status, CASE WHEN sources.summary IS NULL OR trim(sources.summary) = '' THEN 'missing' ELSE 'ready' END),
                        COALESCE(ps.fts_index_status, 'missing'),
                        COALESCE(ps.dense_index_status, 'missing'),
                        COALESCE(ps.semantic_projection_status, 'disabled'),
                        ps.last_summary_receipt_id,
                        ps.last_dense_index_receipt_id,
                        ps.last_projection_receipt_id,
                        COALESCE(ps.last_error, sources.error_message),
                        COALESCE(ps.updated_at, sources.updated_at)
                 FROM sources
                 LEFT JOIN source_processing_state ps ON ps.source_id = sources.id
                 WHERE sources.id = ?1",
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
                        processing_state: Some(source_processing_state_from_row(row, 17)?),
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

    /// Batch fetch sources. Returns a map keyed by source id. Sources that are
    /// missing from the DB are simply absent from the map (caller handles miss).
    pub fn get_sources(&self, source_ids: &[&str]) -> Result<HashMap<String, Source>, GlossError> {
        if source_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders: Vec<String> = (0..source_ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect();
        let sql = format!(
            "SELECT sources.id, sources.source_type, sources.title, sources.original_filename,
                    sources.file_hash, sources.url, sources.file_path,
                    sources.content_text, sources.word_count, sources.metadata,
                    sources.summary, sources.summary_model,
                    sources.status, sources.error_message, sources.selected,
                    sources.created_at, sources.updated_at,
                    COALESCE(ps.source_id, sources.id),
                    COALESCE(ps.lifecycle_status, sources.status),
                    COALESCE(ps.summary_status, CASE WHEN sources.summary IS NULL OR trim(sources.summary) = '' THEN 'missing' ELSE 'ready' END),
                    COALESCE(ps.fts_index_status, 'missing'),
                    COALESCE(ps.dense_index_status, 'missing'),
                    COALESCE(ps.semantic_projection_status, 'disabled'),
                    ps.last_summary_receipt_id,
                    ps.last_dense_index_receipt_id,
                    ps.last_projection_receipt_id,
                    COALESCE(ps.last_error, sources.error_message),
                    COALESCE(ps.updated_at, sources.updated_at)
             FROM sources
             LEFT JOIN source_processing_state ps ON ps.source_id = sources.id
             WHERE sources.id IN ({})",
            placeholders.join(", ")
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = source_ids
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt.query_map(params.as_slice(), |row| {
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
                processing_state: Some(source_processing_state_from_row(row, 17)?),
            })
        })?;
        let mut sources = HashMap::new();
        for row in rows {
            let source = row?;
            sources.insert(source.id.clone(), source);
        }
        Ok(sources)
    }

    /// Delete a source and its chunks (cascade).
    pub fn delete_source(&self, source_id: &str) -> Result<(), GlossError> {
        self.invalidate_turbo_quant_projection_identity()?;
        self.conn
            .execute("DELETE FROM sources WHERE id = ?1", [source_id])?;
        Ok(())
    }

    /// Commit canonical deletion before callers touch rebuildable projections.
    pub fn delete_source_with_projection_invalidation(
        &self,
        notebook_id: &str,
        source_id: &str,
    ) -> Result<(), GlossError> {
        self.with_dense_index_transaction(|db| {
            db.conn.execute(
                "UPDATE semantic_memory_links SET sync_status = 'stale',
                 sync_error = 'Gloss source deleted', synced_at = datetime('now')
                 WHERE notebook_id = ?1 AND source_id = ?2",
                rusqlite::params![notebook_id, source_id],
            )?;
            db.delete_source(source_id)
        })
    }

    /// Canonical retry reset is atomic; failed deletion preserves old mappings.
    pub fn reset_source_for_reingestion(
        &self,
        notebook_id: &str,
        source_id: &str,
    ) -> Result<String, GlossError> {
        self.with_dense_index_transaction(|db| {
            let source_type = db.get_source(source_id)?.source_type;
            db.conn.execute(
                "UPDATE semantic_memory_links SET sync_status = 'stale',
                 sync_error = 'Gloss source reingestion requested', synced_at = datetime('now')
                 WHERE notebook_id = ?1 AND source_id = ?2",
                rusqlite::params![notebook_id, source_id],
            )?;
            db.update_source_status(source_id, "pending", None)?;
            db.delete_chunks_for_source(source_id)?;
            db.update_source_index_status(source_id, Some("missing"), Some("missing"), None)?;
            Ok(source_type)
        })
    }

    /// Persist the exact set of selected sources for this notebook.
    /// Wrapped in a transaction so a crash between unselect-all and re-select
    /// cannot leave all sources unselected.
    pub fn set_selected_sources(&self, selected_ids: &[String]) -> Result<(), GlossError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("UPDATE sources SET selected = 0", [])?;

        if selected_ids.is_empty() {
            tx.commit()?;
            return Ok(());
        }

        let placeholders: Vec<String> = (0..selected_ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect();
        let sql = format!(
            "UPDATE sources SET selected = 1 WHERE id IN ({})",
            placeholders.join(", ")
        );
        let params: Vec<&dyn rusqlite::types::ToSql> = selected_ids
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();
        tx.execute(&sql, params.as_slice())?;
        tx.commit()?;
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
        // Invalidate before canonical mutation. If insertion fails, remaining
        // artifacts may need a rebuild but can never retain false freshness.
        self.invalidate_turbo_quant_projection_identity()?;
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

    /// Insert chunks in one transaction so source ingestion does not autocommit
    /// each chunk row independently.
    pub fn insert_chunks(&self, chunks: &[Chunk]) -> Result<(), GlossError> {
        let tx = self.conn.unchecked_transaction()?;
        if !chunks.is_empty() {
            self.invalidate_turbo_quant_projection_identity()?;
        }
        {
            let mut stmt = tx.prepare(
                "INSERT INTO chunks (id, source_id, chunk_index, content, token_count,
                                     start_offset, end_offset, metadata, embedding_id, embedding_model)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for chunk in chunks {
                stmt.execute(rusqlite::params![
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
                ])?;
            }
        }
        tx.commit()?;
        for source_id in chunks
            .iter()
            .map(|chunk| chunk.source_id.as_str())
            .collect::<std::collections::HashSet<_>>()
        {
            self.update_source_index_status(source_id, Some("indexed"), None, None)?;
        }
        Ok(())
    }

    /// Serialize native artifact writers across pooled and queue connections.
    /// The caller publishes the artifact before updating mappings. A failed
    /// mapping/status write rolls the entire batch back, leaving retryable
    /// unmapped chunks (at worst, harmless orphan vectors in the projection).
    pub fn with_dense_index_transaction<T>(
        &self,
        publish: impl FnOnce(&Self) -> Result<T, GlossError>,
    ) -> Result<T, GlossError> {
        let transaction = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let result = publish(self)?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn native_embedding_ids(&self) -> Result<Vec<i64>, GlossError> {
        let mut statement = self
            .conn
            .prepare("SELECT embedding_id FROM chunks WHERE embedding_id IS NOT NULL")?;
        let ids = statement.query_map([], |row| row.get(0))?;
        ids.collect::<Result<Vec<_>, _>>().map_err(GlossError::from)
    }

    /// Exact canonical snapshot for a whole-notebook native rebuild.
    pub fn native_rebuild_chunks(&self) -> Result<Vec<Chunk>, GlossError> {
        let mut statement = self.conn.prepare("SELECT id, source_id, chunk_index, content, token_count, start_offset, end_offset, metadata, embedding_id, embedding_model FROM chunks ORDER BY id")?;
        let rows = statement.query_map([], |row| {
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
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(GlossError::from)
    }

    /// Only failures explicitly classified by the native embedding stage may
    /// be closed by rebuilding a projection. File/extraction errors stay open.
    pub fn native_embedding_failures(&self) -> Result<Vec<(String, Option<String>)>, GlossError> {
        let mut statement = self.conn.prepare("SELECT s.id, s.error_message FROM sources s JOIN source_processing_state ps ON ps.source_id=s.id WHERE s.status='error' AND ps.fts_index_status='indexed' AND ps.dense_index_status='blocked' AND EXISTS(SELECT 1 FROM chunks c WHERE c.source_id=s.id) ORDER BY s.id")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(GlossError::from)
    }

    pub fn close_rebuilt_embedding_failures(
        &self,
        expected: &[(String, Option<String>)],
    ) -> Result<usize, GlossError> {
        let mut recovered = 0;
        for (id, error) in expected {
            let source = self.get_source(id)?;
            if source.status != "error" || &source.error_message != error {
                continue;
            }
            let incomplete: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM chunks WHERE source_id=?1 AND embedding_id IS NULL)",
                [id],
                |row| row.get(0),
            )?;
            if !incomplete {
                self.update_source_status(id, "ready", None)?;
                recovered += 1;
            }
        }
        Ok(recovered)
    }

    /// Update a mapping only after verified artifact publication, inside the
    /// native writer transaction. Source readiness requires all its chunks.
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
        let source_id: Option<String> = self
            .conn
            .query_row(
                "SELECT source_id FROM chunks WHERE id = ?1",
                [chunk_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(source_id) = source_id {
            let incomplete: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM chunks WHERE source_id = ?1 AND embedding_id IS NULL)",
                [&source_id],
                |row| row.get(0),
            )?;
            self.update_source_index_status(
                &source_id,
                None,
                Some(if incomplete { "building" } else { "indexed" }),
                None,
            )?;
        }
        Ok(())
    }

    pub fn update_source_lifecycle_status(
        &self,
        source_id: &str,
        status: &str,
        error_message: Option<&str>,
    ) -> Result<(), GlossError> {
        self.conn.execute(
            "INSERT INTO source_processing_state (source_id, lifecycle_status, last_error, updated_at)
             VALUES (?1, ?2, ?3, datetime('now'))
             ON CONFLICT(source_id) DO UPDATE SET
                lifecycle_status = excluded.lifecycle_status,
                last_error = excluded.last_error,
                updated_at = excluded.updated_at",
            rusqlite::params![source_id, status, error_message],
        )?;
        Ok(())
    }

    pub fn update_source_index_status(
        &self,
        source_id: &str,
        fts_index_status: Option<&str>,
        dense_index_status: Option<&str>,
        error_message: Option<&str>,
    ) -> Result<(), GlossError> {
        self.conn.execute(
            "INSERT INTO source_processing_state (
                source_id, lifecycle_status, fts_index_status, dense_index_status, last_error, updated_at
             )
             VALUES (
                ?1,
                COALESCE((SELECT status FROM sources WHERE id = ?1), 'pending'),
                COALESCE(?2, 'missing'),
                COALESCE(?3, 'missing'),
                ?4,
                datetime('now')
             )
             ON CONFLICT(source_id) DO UPDATE SET
                fts_index_status = COALESCE(?2, fts_index_status),
                dense_index_status = COALESCE(?3, dense_index_status),
                last_error = COALESCE(?4, last_error),
                updated_at = datetime('now')",
            rusqlite::params![source_id, fts_index_status, dense_index_status, error_message],
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

    /// Batch fetch chunks for multiple sources. Returns a map keyed by source id to
    /// `Vec<Chunk>`, with empty vectors when a source has no chunks.
    pub fn get_chunks_for_sources(
        &self,
        source_ids: &[&str],
    ) -> Result<HashMap<String, Vec<Chunk>>, GlossError> {
        let mut chunks_by_source: HashMap<String, Vec<Chunk>> = source_ids
            .iter()
            .map(|source_id| (source_id.to_string(), Vec::new()))
            .collect();
        if source_ids.is_empty() {
            return Ok(chunks_by_source);
        }

        let placeholders: Vec<String> = (0..source_ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect();
        let sql = format!(
            "SELECT id, source_id, chunk_index, content, token_count, start_offset, end_offset,
                    metadata, embedding_id, embedding_model
             FROM chunks WHERE source_id IN ({}) ORDER BY source_id, chunk_index",
            placeholders.join(", ")
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = source_ids
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
        for row in rows {
            let chunk = row?;
            chunks_by_source
                .entry(chunk.source_id.clone())
                .or_default()
                .push(chunk);
        }
        Ok(chunks_by_source)
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
        let placeholders: Vec<String> = (0..chunk_ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect();
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

    /// Get chunks for a source that don't yet have embeddings.
    pub fn get_chunks_without_embedding(&self, source_id: &str) -> Result<Vec<Chunk>, GlossError> {
        let sql =
            "SELECT id, source_id, chunk_index, content, token_count, start_offset, end_offset,
                    metadata, embedding_id, embedding_model
                 FROM chunks WHERE source_id = ?1 AND embedding_id IS NULL ORDER BY chunk_index";
        let mut stmt = self.conn.prepare(sql)?;
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

    /// Returns true when every chunk in the selected scope has an embedding and
    /// there is at least one embedded chunk available for hybrid search.
    pub fn can_run_hybrid_search(&self, scoped_ids: &[String]) -> Result<bool, GlossError> {
        let (embedded_sql, missing_sql, params): (
            String,
            String,
            Vec<&dyn rusqlite::types::ToSql>,
        ) = if scoped_ids.is_empty() {
            (
                "SELECT COUNT(*) FROM chunks WHERE embedding_id IS NOT NULL".to_string(),
                "SELECT COUNT(*) FROM chunks WHERE embedding_id IS NULL".to_string(),
                Vec::new(),
            )
        } else {
            let placeholders: Vec<String> = (0..scoped_ids.len())
                .map(|i| format!("?{}", i + 1))
                .collect();
            let clause = placeholders.join(", ");
            (
                    format!(
                        "SELECT COUNT(*) FROM chunks WHERE embedding_id IS NOT NULL AND source_id IN ({clause})"
                    ),
                    format!(
                        "SELECT COUNT(*) FROM chunks WHERE embedding_id IS NULL AND source_id IN ({clause})"
                    ),
                    scoped_ids
                        .iter()
                        .map(|id| id as &dyn rusqlite::types::ToSql)
                        .collect(),
                )
        };
        let embedded: i64 = self
            .conn
            .query_row(&embedded_sql, params.as_slice(), |row| row.get(0))?;
        if embedded == 0 {
            return Ok(false);
        }

        let missing: i64 = self
            .conn
            .query_row(&missing_sql, params.as_slice(), |row| row.get(0))?;
        Ok(missing == 0)
    }

    pub fn retrieval_coverage(
        &self,
        scope: &ResolvedSourceScope,
    ) -> Result<RetrievalCoverage, GlossError> {
        if scope.is_none() {
            return Ok(RetrievalCoverage {
                selected_sources: 0,
                total_chunks: 0,
                fts_indexed_chunks: 0,
                embedded_chunks: 0,
                missing_embeddings: 0,
                semantic_links_total: 0,
                semantic_links_healthy: 0,
                semantic_links_degraded: 0,
                dense_coverage_ratio: 0.0,
            });
        }
        let scoped_ids = scope.source_ids();
        if scoped_ids.is_empty() {
            return Ok(RetrievalCoverage {
                selected_sources: 0,
                total_chunks: 0,
                fts_indexed_chunks: 0,
                embedded_chunks: 0,
                missing_embeddings: 0,
                semantic_links_total: 0,
                semantic_links_healthy: 0,
                semantic_links_degraded: 0,
                dense_coverage_ratio: 0.0,
            });
        }
        let (chunk_clause, params): (String, Vec<&dyn rusqlite::types::ToSql>) = {
            let placeholders = (0..scoped_ids.len())
                .map(|idx| format!("?{}", idx + 1))
                .collect::<Vec<_>>()
                .join(", ");
            (
                format!(" WHERE source_id IN ({placeholders})"),
                scoped_ids
                    .iter()
                    .map(|id| id as &dyn rusqlite::types::ToSql)
                    .collect(),
            )
        };

        let total_chunks: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM chunks{chunk_clause}"),
            params.as_slice(),
            |row| row.get(0),
        )?;
        let embedded_chunks: i64 = self.conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM chunks{chunk_clause}{}",
                if chunk_clause.is_empty() {
                    " WHERE embedding_id IS NOT NULL"
                } else {
                    " AND embedding_id IS NOT NULL"
                }
            ),
            params.as_slice(),
            |row| row.get(0),
        )?;
        let fts_indexed_chunks: i64 = self.conn.query_row(
            &format!(
                "SELECT COUNT(*)
                 FROM chunks c
                 JOIN chunks_fts fts ON fts.rowid = c.rowid{}",
                {
                    let placeholders = (0..scoped_ids.len())
                        .map(|idx| format!("?{}", idx + 1))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(" WHERE c.source_id IN ({placeholders})")
                }
            ),
            params.as_slice(),
            |row| row.get(0),
        )?;

        let (semantic_links_total, semantic_links_healthy, semantic_links_degraded) = if self
            .table_exists("semantic_memory_links")?
        {
            let link_scope = {
                let placeholders = (0..scoped_ids.len())
                    .map(|idx| format!("?{}", idx + 1))
                    .collect::<Vec<_>>()
                    .join(", ");
                (
                    format!(" WHERE source_id IN ({placeholders})"),
                    scoped_ids
                        .iter()
                        .map(|id| id as &dyn rusqlite::types::ToSql)
                        .collect::<Vec<_>>(),
                )
            };
            let total: i64 = self.conn.query_row(
                &format!("SELECT COUNT(*) FROM semantic_memory_links{}", link_scope.0),
                link_scope.1.as_slice(),
                |row| row.get(0),
            )?;
            let healthy: i64 = self.conn.query_row(
                    &format!(
                        "SELECT COUNT(*) FROM semantic_memory_links{}{}",
                        link_scope.0,
                        " AND sync_status = 'synced' AND sm_document_id IS NOT NULL AND sm_chunk_id IS NOT NULL AND content_digest != ''"
                    ),
                    link_scope.1.as_slice(),
                    |row| row.get(0),
                )?;
            let projection_unit_count: i64 = self.conn.query_row(
                &format!(
                    "SELECT COUNT(*) FROM semantic_memory_links{}{}",
                    link_scope.0,
                    " AND projection_unit_kind IS NOT NULL AND projection_unit_kind != 'chunk'"
                ),
                link_scope.1.as_slice(),
                |row| row.get(0),
            )?;
            let healthy_parent_link_count = healthy.saturating_sub(projection_unit_count).max(0);
            let _projection_unit_summary = (projection_unit_count, healthy_parent_link_count);
            (
                total.max(0) as usize,
                healthy.max(0) as usize,
                total.saturating_sub(healthy).max(0) as usize,
            )
        } else {
            (0, 0, 0)
        };

        let total_chunks = total_chunks.max(0) as usize;
        let embedded_chunks = embedded_chunks.max(0) as usize;
        let missing_embeddings = total_chunks.saturating_sub(embedded_chunks);
        Ok(RetrievalCoverage {
            selected_sources: scoped_ids.len(),
            total_chunks,
            fts_indexed_chunks: fts_indexed_chunks.max(0) as usize,
            embedded_chunks,
            missing_embeddings,
            semantic_links_total,
            semantic_links_healthy,
            semantic_links_degraded,
            dense_coverage_ratio: if total_chunks == 0 {
                0.0
            } else {
                embedded_chunks as f64 / total_chunks as f64
            },
        })
    }

    pub fn upsert_semantic_memory_projection_status(
        &self,
        update: &SemanticMemoryProjectionStatusUpdate,
    ) -> Result<(), GlossError> {
        self.conn.execute(
            "INSERT INTO semantic_memory_projection_status
             (notebook_id, source_id, status, chunk_count, projected_chunk_count,
              healthy_link_count, degraded_link_count, last_receipt_id, last_error,
              artifact_generation_id, vector_artifact_manifest_digest, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, datetime('now'))
             ON CONFLICT(notebook_id, source_id) DO UPDATE SET
                status = excluded.status,
                chunk_count = excluded.chunk_count,
                projected_chunk_count = excluded.projected_chunk_count,
                healthy_link_count = excluded.healthy_link_count,
                degraded_link_count = excluded.degraded_link_count,
                last_receipt_id = excluded.last_receipt_id,
                last_error = excluded.last_error,
                artifact_generation_id = excluded.artifact_generation_id,
                vector_artifact_manifest_digest = excluded.vector_artifact_manifest_digest,
                updated_at = excluded.updated_at",
            rusqlite::params![
                update.notebook_id,
                update.source_id,
                update.status,
                update.chunk_count as i64,
                update.projected_chunk_count as i64,
                update.healthy_link_count as i64,
                update.degraded_link_count as i64,
                update.last_receipt_id,
                update.last_error,
                update.artifact_generation_id,
                update.vector_artifact_manifest_digest,
            ],
        )?;
        self.conn.execute(
            "INSERT INTO source_processing_state (
                source_id, lifecycle_status, semantic_projection_status,
                last_projection_receipt_id, last_error, updated_at
             )
             VALUES (
                ?1,
                COALESCE((SELECT status FROM sources WHERE id = ?1), 'ready'),
                ?2,
                ?3,
                ?4,
                datetime('now')
             )
             ON CONFLICT(source_id) DO UPDATE SET
                semantic_projection_status = excluded.semantic_projection_status,
                last_projection_receipt_id = excluded.last_projection_receipt_id,
                last_error = COALESCE(excluded.last_error, last_error),
                updated_at = excluded.updated_at",
            rusqlite::params![
                update.source_id,
                update.status,
                update.last_receipt_id,
                update.last_error,
            ],
        )?;
        Ok(())
    }

    pub fn get_semantic_memory_projection_status(
        &self,
        notebook_id: &str,
        source_id: &str,
    ) -> Result<Option<SemanticMemoryProjectionStatus>, GlossError> {
        if !self.table_exists("semantic_memory_projection_status")? {
            return Ok(None);
        }
        self.conn
            .query_row(
                "SELECT notebook_id, source_id, status, chunk_count, projected_chunk_count,
                        healthy_link_count, degraded_link_count, last_receipt_id, last_error,
                        artifact_generation_id, vector_artifact_manifest_digest, updated_at
                 FROM semantic_memory_projection_status
                 WHERE notebook_id = ?1 AND source_id = ?2",
                rusqlite::params![notebook_id, source_id],
                projection_status_from_row,
            )
            .optional()
            .map_err(GlossError::Database)
    }

    pub fn list_semantic_memory_projection_statuses(
        &self,
        notebook_id: &str,
    ) -> Result<Vec<SemanticMemoryProjectionStatus>, GlossError> {
        if !self.table_exists("semantic_memory_projection_status")? {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT notebook_id, source_id, status, chunk_count, projected_chunk_count,
                    healthy_link_count, degraded_link_count, last_receipt_id, last_error,
                    artifact_generation_id, vector_artifact_manifest_digest, updated_at
             FROM semantic_memory_projection_status
             WHERE notebook_id = ?1
             ORDER BY updated_at DESC, source_id ASC",
        )?;
        let rows = stmt.query_map([notebook_id], projection_status_from_row)?;
        let mut statuses = Vec::new();
        for row in rows {
            statuses.push(row?);
        }
        Ok(statuses)
    }

    pub fn update_semantic_memory_projection_artifact(
        &self,
        notebook_id: &str,
        receipt_id: Option<&str>,
        artifact_generation_id: Option<&str>,
        vector_artifact_manifest_digest: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), GlossError> {
        self.conn.execute(
            "UPDATE semantic_memory_projection_status
             SET last_receipt_id = COALESCE(?2, last_receipt_id),
                 artifact_generation_id = ?3,
                 vector_artifact_manifest_digest = ?4,
                 last_error = ?5,
                 status = CASE
                    WHEN status = 'synced' AND (?3 IS NULL OR ?4 IS NULL) THEN 'artifact_stale'
                    WHEN status = 'artifact_stale' AND ?3 IS NOT NULL AND ?4 IS NOT NULL
                         AND healthy_link_count >= chunk_count AND degraded_link_count = 0
                         AND projected_chunk_count >= chunk_count THEN 'synced'
                    ELSE status
                 END,
                 updated_at = datetime('now')
             WHERE notebook_id = ?1",
            rusqlite::params![
                notebook_id,
                receipt_id,
                artifact_generation_id,
                vector_artifact_manifest_digest,
                error
            ],
        )?;
        Ok(())
    }

    #[allow(clippy::type_complexity)]
    pub fn semantic_memory_projection_summary(
        &self,
        notebook_id: &str,
        scope: &ResolvedSourceScope,
    ) -> Result<SemanticMemoryProjectionSummary, GlossError> {
        if scope.is_none() {
            return Ok(SemanticMemoryProjectionSummary {
                notebook_id: notebook_id.to_string(),
                total_sources: 0,
                chunk_bearing_sources: 0,
                zero_chunk_sources: 0,
                projected_sources: 0,
                failed_sources: 0,
                skipped_no_chunks: 0,
                stale_sources: 0,
                partial_sources: 0,
                projecting_sources: 0,
                healthy_links: 0,
                degraded_links: 0,
                missing_links: 0,
                total_chunks: 0,
                projected_chunks: 0,
                projection_required: false,
            });
        }
        let scoped_ids = scope.source_ids();
        if scoped_ids.is_empty() {
            return Ok(SemanticMemoryProjectionSummary {
                notebook_id: notebook_id.to_string(),
                total_sources: 0,
                chunk_bearing_sources: 0,
                zero_chunk_sources: 0,
                projected_sources: 0,
                failed_sources: 0,
                skipped_no_chunks: 0,
                stale_sources: 0,
                partial_sources: 0,
                projecting_sources: 0,
                healthy_links: 0,
                degraded_links: 0,
                missing_links: 0,
                total_chunks: 0,
                projected_chunks: 0,
                projection_required: false,
            });
        }
        if !self.table_exists("semantic_memory_projection_status")? {
            return Ok(SemanticMemoryProjectionSummary {
                notebook_id: notebook_id.to_string(),
                total_sources: 0,
                chunk_bearing_sources: 0,
                zero_chunk_sources: 0,
                projected_sources: 0,
                failed_sources: 0,
                skipped_no_chunks: 0,
                stale_sources: 0,
                partial_sources: 0,
                projecting_sources: 0,
                healthy_links: 0,
                degraded_links: 0,
                missing_links: 0,
                total_chunks: 0,
                projected_chunks: 0,
                projection_required: false,
            });
        }
        let placeholders = (0..scoped_ids.len())
            .map(|idx| format!("?{}", idx + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let source_filter = format!(" WHERE s.id IN ({placeholders})");

        let sql = format!(
            "SELECT
                COUNT(*) AS total_sources,
                COALESCE(SUM(CASE WHEN COALESCE(chunk_counts.chunk_count, 0) > 0 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN COALESCE(chunk_counts.chunk_count, 0) = 0 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN ps.status = 'synced' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN ps.status = 'failed' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN COALESCE(chunk_counts.chunk_count, 0) = 0 OR ps.status = 'skipped_no_chunks' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN ps.status IN ('stale', 'artifact_stale') THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN ps.status = 'partial' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN ps.status = 'projecting' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(COALESCE(ps.healthy_link_count, 0)), 0),
                COALESCE(SUM(COALESCE(ps.degraded_link_count, 0)), 0),
                COALESCE(SUM(COALESCE(chunk_counts.chunk_count, 0)), 0),
                COALESCE(SUM(COALESCE(ps.projected_chunk_count, 0)), 0)
             FROM sources s
             LEFT JOIN (
                SELECT source_id, COUNT(*) AS chunk_count
                FROM chunks
                GROUP BY source_id
             ) chunk_counts ON chunk_counts.source_id = s.id
             LEFT JOIN semantic_memory_projection_status ps
                ON ps.source_id = s.id AND ps.notebook_id = ?1{source_filter}"
        );

        let mut params: Vec<&dyn rusqlite::types::ToSql> = vec![&notebook_id];
        for source_id in scoped_ids {
            params.push(source_id);
        }

        let (
            total_sources,
            chunk_bearing_sources,
            zero_chunk_sources,
            projected_sources,
            failed_sources,
            skipped_no_chunks,
            stale_sources,
            partial_sources,
            projecting_sources,
            healthy_links,
            degraded_links,
            total_chunks,
            projected_chunks,
        ): (
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
        ) = self.conn.query_row(&sql, params.as_slice(), |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
                row.get(11)?,
                row.get(12)?,
            ))
        })?;

        let total_sources = total_sources.max(0) as usize;
        let chunk_bearing_sources = chunk_bearing_sources.max(0) as usize;
        let zero_chunk_sources = zero_chunk_sources.max(0) as usize;
        let projected_sources = projected_sources.max(0) as usize;
        let failed_sources = failed_sources.max(0) as usize;
        let skipped_no_chunks = skipped_no_chunks.max(0) as usize;
        let stale_sources = stale_sources.max(0) as usize;
        let partial_sources = partial_sources.max(0) as usize;
        let projecting_sources = projecting_sources.max(0) as usize;
        let healthy_links = healthy_links.max(0) as usize;
        let degraded_links = degraded_links.max(0) as usize;
        let total_chunks = total_chunks.max(0) as usize;
        let projected_chunks = projected_chunks.max(0) as usize;
        let missing_links = total_chunks.saturating_sub(healthy_links);
        let projection_required = chunk_bearing_sources > 0
            && (healthy_links < total_chunks
                || failed_sources > 0
                || stale_sources > 0
                || partial_sources > 0
                || projecting_sources > 0);

        Ok(SemanticMemoryProjectionSummary {
            notebook_id: notebook_id.to_string(),
            total_sources,
            chunk_bearing_sources,
            zero_chunk_sources,
            projected_sources,
            failed_sources,
            skipped_no_chunks,
            stale_sources,
            partial_sources,
            projecting_sources,
            healthy_links,
            degraded_links,
            missing_links,
            total_chunks,
            projected_chunks,
            projection_required,
        })
    }

    pub fn table_exists(&self, table: &str) -> Result<bool, GlossError> {
        let exists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )?;
        Ok(exists > 0)
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
                rusqlite::Error::QueryReturnedNoRows => GlossError::NotFound(format!(
                    "Chunk with embedding_id {embedding_id} not found"
                )),
                other => GlossError::Database(other),
            })
    }

    /// Batch fetch chunks by embedding IDs. Returns a map keyed by embedding ID.
    pub fn get_chunks_by_embedding_ids(
        &self,
        embedding_ids: &[i64],
    ) -> Result<HashMap<i64, Chunk>, GlossError> {
        if embedding_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders: Vec<String> = (0..embedding_ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect();
        let sql = format!(
            "SELECT id, source_id, chunk_index, content, token_count, start_offset, end_offset,
                    metadata, embedding_id, embedding_model
             FROM chunks WHERE embedding_id IN ({})",
            placeholders.join(", ")
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = embedding_ids
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok((
                row.get::<_, i64>(8)?,
                Chunk {
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
                },
            ))
        })?;
        let mut chunks = HashMap::new();
        for row in rows {
            let (embedding_id, chunk) = row?;
            chunks.insert(embedding_id, chunk);
        }
        Ok(chunks)
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

    /// FTS5 search for chunks constrained to a resolved source scope.
    pub fn fts_search_chunks_in_sources(
        &self,
        query: &str,
        scope: &ResolvedSourceScope,
        limit: usize,
    ) -> Result<Vec<ChunkSearchHit>, GlossError> {
        if limit == 0 || scope.is_none() || scope.source_ids().is_empty() {
            return Ok(Vec::new());
        }
        let source_ids: Vec<&str> = scope.source_ids().iter().map(String::as_str).collect();
        self.fts_search_chunks_in_sources_batched(query, &source_ids, limit)
    }

    pub fn fts_search_chunks_in_sources_batched(
        &self,
        query: &str,
        source_ids: &[&str],
        limit: usize,
    ) -> Result<Vec<ChunkSearchHit>, GlossError> {
        if limit == 0 || source_ids.is_empty() {
            return Ok(Vec::new());
        }

        let capped_ids: Vec<&str> = source_ids.iter().copied().take(256).collect();
        if capped_ids.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders: Vec<String> = (0..capped_ids.len())
            .map(|i| format!("?{}", i + 2))
            .collect();
        let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::with_capacity(capped_ids.len() + 2);
        params.push(&query);
        for source_id in &capped_ids {
            params.push(source_id);
        }
        let limit = limit as i64;
        let limit_placeholder = capped_ids.len() + 2;
        let sql = format!(
            "SELECT c.id, c.source_id, c.chunk_index, c.content, c.token_count,
                    c.start_offset, c.end_offset, c.metadata, c.embedding_id,
                    c.embedding_model, chunks_fts.rank
             FROM chunks_fts
             JOIN chunks c ON c.rowid = chunks_fts.rowid
             WHERE chunks_fts MATCH ?1 AND c.source_id IN ({})
             ORDER BY chunks_fts.rank ASC
             LIMIT ?{}",
            placeholders.join(", "),
            limit_placeholder
        );
        params.push(&limit);
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok((
                Chunk {
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
                },
                row.get::<_, f64>(10)?,
            ))
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        results.sort_by(|(a_chunk, a_score), (b_chunk, b_score)| {
            a_score
                .partial_cmp(b_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a_chunk.source_id.cmp(&b_chunk.source_id))
                .then_with(|| a_chunk.chunk_index.cmp(&b_chunk.chunk_index))
                .then_with(|| a_chunk.id.cmp(&b_chunk.id))
        });
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
        self.conn
            .execute("INSERT INTO conversations (id) VALUES (?1)", [id])?;
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

    pub fn upsert_chat_attempt_status(
        &self,
        attempt: &ChatAttemptStatus,
    ) -> Result<(), GlossError> {
        self.conn.execute(
            "INSERT INTO chat_attempts (
                attempt_id, notebook_id, conversation_id, assistant_message_id,
                user_message_id, provider, model, status, phase, error_code,
                error_message, request_digest, response_digest, partial_policy,
                created_at, updated_at, terminal_at
             )
             VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                datetime('now'), datetime('now'), CASE WHEN ?15 THEN datetime('now') ELSE NULL END
             )
             ON CONFLICT(attempt_id) DO UPDATE SET
                notebook_id = excluded.notebook_id,
                conversation_id = excluded.conversation_id,
                assistant_message_id = excluded.assistant_message_id,
                user_message_id = COALESCE(excluded.user_message_id, chat_attempts.user_message_id),
                provider = COALESCE(excluded.provider, chat_attempts.provider),
                model = COALESCE(excluded.model, chat_attempts.model),
                status = excluded.status,
                phase = excluded.phase,
                error_code = excluded.error_code,
                error_message = excluded.error_message,
                request_digest = COALESCE(excluded.request_digest, chat_attempts.request_digest),
                response_digest = COALESCE(excluded.response_digest, chat_attempts.response_digest),
                partial_policy = COALESCE(excluded.partial_policy, chat_attempts.partial_policy),
                updated_at = datetime('now'),
                terminal_at = CASE WHEN ?15 THEN datetime('now') ELSE chat_attempts.terminal_at END",
            rusqlite::params![
                attempt.attempt_id,
                attempt.notebook_id,
                attempt.conversation_id,
                attempt.assistant_message_id,
                attempt.user_message_id,
                attempt.provider,
                attempt.model,
                attempt.status,
                attempt.phase,
                attempt.error_code,
                attempt.error_message,
                attempt.request_digest,
                attempt.response_digest,
                attempt.partial_policy,
                attempt.terminal,
            ],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_prompt_receipt(
        &self,
        receipt_id: &str,
        notebook_id: &str,
        conversation_id: &str,
        message_id: &str,
        prompt_digest: &str,
        context_payload_digest: &str,
        raw_receipt_json: &str,
    ) -> Result<(), GlossError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO prompt_receipts
             (receipt_id, notebook_id, conversation_id, message_id, prompt_digest,
              context_payload_digest, raw_receipt_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                receipt_id,
                notebook_id,
                conversation_id,
                message_id,
                prompt_digest,
                context_payload_digest,
                raw_receipt_json
            ],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_generation_receipt(
        &self,
        receipt_id: &str,
        notebook_id: &str,
        conversation_id: &str,
        message_id: &str,
        provider: &str,
        model: &str,
        provider_request_digest: &str,
        response_digest: Option<&str>,
        status: &str,
        raw_receipt_json: &str,
    ) -> Result<(), GlossError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO generation_receipts
             (receipt_id, notebook_id, conversation_id, message_id, provider, model,
              provider_request_digest, response_digest, status, raw_receipt_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                receipt_id,
                notebook_id,
                conversation_id,
                message_id,
                provider,
                model,
                provider_request_digest,
                response_digest,
                status,
                raw_receipt_json
            ],
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

    // -- Studio outputs --

    pub fn list_studio_outputs(&self) -> Result<Vec<StudioOutput>, GlossError> {
        let mut stmt = match self.conn.prepare(
            "SELECT id, output_type, title, prompt_used, raw_content, config, source_ids,
                    file_path, status, error_message, prose_content, created_at
             FROM studio_outputs
             ORDER BY created_at DESC",
        ) {
            Ok(s) => s,
            Err(e) => {
                if format!("{e}").contains("no such table") {
                    tracing::warn!(
                        "studio_outputs table missing — returning empty list (run ensure_studio_outputs)"
                    );
                    return Ok(Vec::new());
                }
                return Err(e.into());
            }
        };
        let rows = match stmt.query_map([], studio_output_from_row) {
            Ok(r) => r,
            Err(e) => {
                if format!("{e}").contains("no such table") {
                    tracing::warn!("studio_outputs table missing on query_map");
                    return Ok(Vec::new());
                }
                return Err(e.into());
            }
        };
        let mut outputs = Vec::new();
        for row in rows {
            outputs.push(row?);
        }
        Ok(outputs)
    }

    pub fn get_studio_output(&self, output_id: &str) -> Result<StudioOutput, GlossError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, output_type, title, prompt_used, raw_content, config, source_ids,
                    file_path, status, error_message, prose_content, created_at
             FROM studio_outputs
             WHERE id = ?1",
        )?;
        stmt.query_row([output_id], studio_output_from_row)
            .map_err(GlossError::from)
    }

    pub fn insert_studio_output(&self, output: &StudioOutput) -> Result<(), GlossError> {
        self.conn.execute(
            "INSERT INTO studio_outputs (
                id, output_type, title, prompt_used, raw_content, config,
                source_ids, file_path, status, error_message, prose_content
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                output.id,
                output.output_type,
                output.title,
                output.prompt_used,
                output.raw_content,
                output.config,
                output.source_ids,
                output.file_path,
                output.status,
                output.error_message,
                output.prose_content,
            ],
        )?;
        Ok(())
    }

    pub fn update_studio_output_file_path(
        &self,
        output_id: &str,
        file_path: &str,
    ) -> Result<(), GlossError> {
        self.conn.execute(
            "UPDATE studio_outputs SET file_path = ?1 WHERE id = ?2",
            rusqlite::params![file_path, output_id],
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
                processing_state: None,
            })
        })?;
        let mut sources = Vec::new();
        for row in rows {
            sources.push(row?);
        }
        Ok(sources)
    }

    /// List ready sources missing summaries without loading `content_text`.
    pub fn list_source_headers_needing_summary(
        &self,
    ) -> Result<Vec<SourceSummaryCandidate>, GlossError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title
             FROM sources
             WHERE status = 'ready' AND summary IS NULL
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SourceSummaryCandidate {
                id: row.get(0)?,
                title: row.get(1)?,
            })
        })?;
        let mut sources = Vec::new();
        for row in rows {
            sources.push(row?);
        }
        Ok(sources)
    }

    /// Check if a source with the given file hash already exists.
    pub fn source_exists_by_hash(&self, hash: &str) -> Result<bool, GlossError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sources WHERE file_hash = ?1",
            [hash],
            |row| row.get(0),
        )?;
        Ok(count > 0)
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

    /// Get the maximum embedding_id across all chunks (high-water mark for HNSW labels).
    pub fn max_embedding_id(&self) -> Result<Option<i64>, GlossError> {
        let result: Option<i64> =
            self.conn
                .query_row("SELECT MAX(embedding_id) FROM chunks", [], |row| row.get(0))?;
        Ok(result)
    }

    pub fn embedding_index_metadata(
        &self,
        index_id: &str,
    ) -> Result<Option<EmbeddingIndexMetadata>, GlossError> {
        self.conn
            .query_row(
                "SELECT index_id, provider, model, model_digest, dimensions, schema_version,
                        status, status_reason, created_at, validated_at
                 FROM embedding_index_metadata
                 WHERE index_id = ?1",
                rusqlite::params![index_id],
                |row| {
                    let dimensions: Option<i64> = row.get(4)?;
                    Ok(EmbeddingIndexMetadata {
                        index_id: row.get(0)?,
                        provider: row.get(1)?,
                        model: row.get(2)?,
                        model_digest: row.get(3)?,
                        dimensions: dimensions.and_then(|value| usize::try_from(value).ok()),
                        schema_version: row.get(5)?,
                        status: row.get(6)?,
                        status_reason: row.get(7)?,
                        created_at: row.get(8)?,
                        validated_at: row.get(9)?,
                    })
                },
            )
            .optional()
            .map_err(GlossError::from)
    }

    pub fn list_embedding_index_metadata(&self) -> Result<Vec<EmbeddingIndexMetadata>, GlossError> {
        let mut stmt = self.conn.prepare(
            "SELECT index_id, provider, model, model_digest, dimensions, schema_version,
                    status, status_reason, created_at, validated_at
             FROM embedding_index_metadata
             ORDER BY index_id",
        )?;
        let rows = stmt.query_map([], |row| {
            let dimensions: Option<i64> = row.get(4)?;
            Ok(EmbeddingIndexMetadata {
                index_id: row.get(0)?,
                provider: row.get(1)?,
                model: row.get(2)?,
                model_digest: row.get(3)?,
                dimensions: dimensions.and_then(|value| usize::try_from(value).ok()),
                schema_version: row.get(5)?,
                status: row.get(6)?,
                status_reason: row.get(7)?,
                created_at: row.get(8)?,
                validated_at: row.get(9)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn upsert_embedding_index_metadata(
        &self,
        metadata: &EmbeddingIndexMetadata,
    ) -> Result<(), GlossError> {
        let dimensions = metadata
            .dimensions
            .map(|value| i64::try_from(value).unwrap_or(i64::MAX));
        self.conn.execute(
            "INSERT INTO embedding_index_metadata
             (index_id, provider, model, model_digest, dimensions, schema_version,
              status, status_reason, created_at, validated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, datetime('now'), datetime('now'))
             ON CONFLICT(index_id) DO UPDATE SET
                provider = excluded.provider,
                model = excluded.model,
                model_digest = excluded.model_digest,
                dimensions = excluded.dimensions,
                schema_version = excluded.schema_version,
                status = excluded.status,
                status_reason = excluded.status_reason,
                validated_at = datetime('now')",
            rusqlite::params![
                metadata.index_id,
                metadata.provider,
                metadata.model,
                metadata.model_digest,
                dimensions,
                metadata.schema_version,
                metadata.status,
                metadata.status_reason,
            ],
        )?;
        Ok(())
    }

    pub fn mark_embedding_index_status(
        &self,
        index_id: &str,
        status: EmbeddingIndexMetadataStatus,
        reason: Option<&str>,
    ) -> Result<(), GlossError> {
        if index_id == SEMANTIC_MEMORY_INDEX_ID && status != EmbeddingIndexMetadataStatus::Ready {
            self.invalidate_turbo_quant_projection_identity()?;
        }
        self.conn.execute(
            "INSERT INTO embedding_index_metadata
             (index_id, provider, model, model_digest, dimensions, schema_version,
              status, status_reason, created_at, validated_at)
             VALUES (?1, '', '', NULL, NULL, ?2, ?3, ?4, datetime('now'), datetime('now'))
             ON CONFLICT(index_id) DO UPDATE SET
                status = excluded.status,
                status_reason = excluded.status_reason,
                validated_at = datetime('now')",
            rusqlite::params![
                index_id,
                EMBEDDING_INDEX_SCHEMA_VERSION,
                status.as_str(),
                reason
            ],
        )?;
        Ok(())
    }

    /// Delete all chunks for a source.
    pub fn delete_chunks_for_source(&self, source_id: &str) -> Result<(), GlossError> {
        self.invalidate_turbo_quant_projection_identity()?;
        self.conn
            .execute("DELETE FROM chunks WHERE source_id = ?1", [source_id])?;
        Ok(())
    }

    /// Canonical corpus changes invalidate every derived artifact generation.
    /// NotebookDb owns one notebook, so this cannot widen to other notebooks.
    fn invalidate_turbo_quant_projection_identity(&self) -> Result<(), GlossError> {
        self.conn.execute(
            "UPDATE semantic_memory_projection_status
             SET artifact_generation_id = NULL, vector_artifact_manifest_digest = NULL,
                 status = CASE WHEN status = 'synced' THEN 'artifact_stale' ELSE status END,
                 updated_at = datetime('now')
             WHERE artifact_generation_id IS NOT NULL OR vector_artifact_manifest_digest IS NOT NULL",
            [],
        )?;
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
    pub fn get_selected_summaries(
        &self,
    ) -> Result<Vec<(String, String, Option<String>)>, GlossError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, summary FROM sources WHERE selected = 1 AND summary IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
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
            processing_state: None,
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
    fn test_set_selected_sources() {
        let db = test_db();
        for id in ["s1", "s2"] {
            db.insert_source(&Source {
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

        db.set_selected_sources(&["s2".to_string()]).unwrap();
        let selected = db.get_selected_source_ids().unwrap();
        assert_eq!(selected, vec!["s2".to_string()]);
    }

    #[test]
    fn get_sources_batched_with_multiple_ids() {
        let db = test_db();
        for id in ["s1", "s2", "s3"] {
            db.insert_source(&Source {
                id: id.to_string(),
                source_type: "text".to_string(),
                title: format!("Source {id}"),
                original_filename: None,
                file_hash: None,
                url: None,
                file_path: None,
                content_text: None,
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
            })
            .unwrap();
        }

        let source_ids = ["s1", "s3"];
        let sources = db.get_sources(&source_ids).unwrap();
        assert_eq!(sources.len(), 2);
        assert!(sources.contains_key("s1"));
        assert!(sources.contains_key("s3"));
        assert!(!sources.contains_key("s2"));
    }

    #[test]
    fn get_chunks_by_embedding_ids_batched_with_multiple_ids() {
        let db = test_db();
        db.insert_source(&Source {
            id: "s1".to_string(),
            source_type: "text".to_string(),
            title: "Source".to_string(),
            original_filename: None,
            file_hash: None,
            url: None,
            file_path: None,
            content_text: None,
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
        })
        .unwrap();

        db.insert_chunk(&Chunk {
            id: "c1".to_string(),
            source_id: "s1".to_string(),
            chunk_index: 0,
            content: "first".to_string(),
            token_count: None,
            start_offset: None,
            end_offset: None,
            metadata: None,
            embedding_id: Some(11),
            embedding_model: None,
        })
        .unwrap();
        db.insert_chunk(&Chunk {
            id: "c2".to_string(),
            source_id: "s1".to_string(),
            chunk_index: 1,
            content: "second".to_string(),
            token_count: None,
            start_offset: None,
            end_offset: None,
            metadata: None,
            embedding_id: Some(22),
            embedding_model: None,
        })
        .unwrap();

        let chunks = db.get_chunks_by_embedding_ids(&[11, 22]).unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks.get(&11).map(|chunk| chunk.id.as_str()), Some("c1"));
        assert_eq!(chunks.get(&22).map(|chunk| chunk.id.as_str()), Some("c2"));
    }

    #[test]
    fn embedding_index_metadata_round_trips_and_marks_stale() {
        let dir = tempdir().unwrap();
        let db = NotebookDb::open(&dir.path().join("notebook.db")).unwrap();
        let metadata = EmbeddingIndexMetadata::ready(
            NATIVE_HNSW_INDEX_ID,
            "ollama",
            "all-minilm",
            Some("digest-384".to_string()),
            384,
        );

        db.upsert_embedding_index_metadata(&metadata).unwrap();
        let stored = db
            .embedding_index_metadata(NATIVE_HNSW_INDEX_ID)
            .unwrap()
            .expect("metadata row should exist");
        assert!(stored.identity_matches(&metadata));
        assert_eq!(stored.dimensions, Some(384));

        db.mark_embedding_index_status(
            NATIVE_HNSW_INDEX_ID,
            EmbeddingIndexMetadataStatus::Stale,
            Some("model switched to 1024d"),
        )
        .unwrap();
        let stale = db
            .embedding_index_metadata(NATIVE_HNSW_INDEX_ID)
            .unwrap()
            .expect("metadata row should still exist");
        assert_eq!(stale.status, EmbeddingIndexMetadataStatus::Stale.as_str());
        assert_eq!(
            stale.status_reason.as_deref(),
            Some("model switched to 1024d")
        );
    }

    #[test]
    fn get_chunks_for_sources_batched_with_empty_and_populated_sources() {
        let db = test_db();
        for id in ["s1", "s2", "s3"] {
            db.insert_source(&Source {
                id: id.to_string(),
                source_type: "text".to_string(),
                title: format!("Source {id}"),
                original_filename: None,
                file_hash: None,
                url: None,
                file_path: None,
                content_text: None,
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
            })
            .unwrap();
        }

        db.insert_chunk(&Chunk {
            id: "c1".to_string(),
            source_id: "s1".to_string(),
            chunk_index: 0,
            content: "first".to_string(),
            token_count: None,
            start_offset: None,
            end_offset: None,
            metadata: None,
            embedding_id: None,
            embedding_model: None,
        })
        .unwrap();
        db.insert_chunk(&Chunk {
            id: "c2".to_string(),
            source_id: "s1".to_string(),
            chunk_index: 1,
            content: "second".to_string(),
            token_count: None,
            start_offset: None,
            end_offset: None,
            metadata: None,
            embedding_id: None,
            embedding_model: None,
        })
        .unwrap();

        let chunks_by_source = db.get_chunks_for_sources(&["s1", "s2", "s3"]).unwrap();
        assert_eq!(chunks_by_source.len(), 3);
        assert_eq!(chunks_by_source["s1"].len(), 2);
        assert_eq!(chunks_by_source["s2"].len(), 0);
        assert_eq!(chunks_by_source["s3"].len(), 0);
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
            processing_state: None,
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
    fn scoped_fts_search_filters_sources_limits_and_orders_deterministically() {
        let db = test_db();
        for id in ["a", "b"] {
            db.insert_source(&Source {
                id: id.to_string(),
                source_type: "text".to_string(),
                title: id.to_string(),
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
                processing_state: None,
            })
            .unwrap();
        }

        for (id, source_id, chunk_index, content) in [
            ("a-02", "a", 2, "tie"),
            ("a-01", "a", 1, "tie"),
            ("b-00", "b", 0, "tie tie tie outside scope"),
            ("a-03", "a", 3, "tie"),
        ] {
            db.insert_chunk(&Chunk {
                id: id.to_string(),
                source_id: source_id.to_string(),
                chunk_index,
                content: content.to_string(),
                token_count: None,
                start_offset: None,
                end_offset: None,
                metadata: None,
                embedding_id: None,
                embedding_model: None,
            })
            .unwrap();
        }

        let source_ids = vec!["a".to_string()];
        let scope = crate::retrieval::source_scope::SourceScope::Explicit(source_ids)
            .resolve(&db.list_sources().unwrap());
        let limited = db.fts_search_chunks_in_sources("tie", &scope, 2).unwrap();
        let limited_ids = limited
            .iter()
            .map(|(chunk, _rank)| chunk.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(limited_ids, vec!["a-01", "a-02"]);
        assert!(limited.iter().all(|(chunk, _rank)| chunk.source_id == "a"));

        let all_scoped = db.fts_search_chunks_in_sources("tie", &scope, 10).unwrap();
        let all_ids = all_scoped
            .iter()
            .map(|(chunk, _rank)| chunk.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(all_ids, vec!["a-01", "a-02", "a-03"]);
    }

    #[test]
    fn retrieval_coverage_distinguishes_none_all_and_explicit() {
        let db = test_db();
        for id in ["a", "b"] {
            db.insert_source(&Source {
                id: id.to_string(),
                source_type: "text".to_string(),
                title: id.to_string(),
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
                processing_state: None,
            })
            .unwrap();
            db.insert_chunk(&Chunk {
                id: format!("c-{id}"),
                source_id: id.to_string(),
                chunk_index: 0,
                content: format!("coverage {id}"),
                token_count: None,
                start_offset: None,
                end_offset: None,
                metadata: None,
                embedding_id: None,
                embedding_model: None,
            })
            .unwrap();
        }

        let sources = db.list_sources().unwrap();
        let none = crate::retrieval::source_scope::SourceScope::None.resolve(&sources);
        let all = crate::retrieval::source_scope::SourceScope::All.resolve(&sources);
        let explicit = crate::retrieval::source_scope::SourceScope::Explicit(vec!["a".to_string()])
            .resolve(&sources);

        assert_eq!(db.retrieval_coverage(&none).unwrap().total_chunks, 0);
        assert_eq!(db.retrieval_coverage(&all).unwrap().total_chunks, 2);
        assert_eq!(db.retrieval_coverage(&explicit).unwrap().total_chunks, 1);
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
            processing_state: None,
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

    #[test]
    fn semantic_memory_projection_summary_marks_projection_required_until_links_healthy() {
        let db = test_db();
        let source = Source {
            id: "s-proj".to_string(),
            source_type: "text".to_string(),
            title: "Projection".to_string(),
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
        };
        db.insert_source(&source).unwrap();
        db.insert_chunk(&Chunk {
            id: "c-proj".to_string(),
            source_id: "s-proj".to_string(),
            chunk_index: 0,
            content: "semantic content".to_string(),
            token_count: Some(2),
            start_offset: None,
            end_offset: None,
            metadata: None,
            embedding_id: None,
            embedding_model: None,
        })
        .unwrap();

        let summary = db
            .semantic_memory_projection_summary(
                "notebook-proj",
                &crate::retrieval::source_scope::SourceScope::All
                    .resolve(&db.list_sources().unwrap()),
            )
            .unwrap();
        assert_eq!(summary.chunk_bearing_sources, 1);
        assert_eq!(summary.total_chunks, 1);
        assert_eq!(summary.healthy_links, 0);
        assert!(summary.projection_required);

        db.upsert_semantic_memory_projection_status(&SemanticMemoryProjectionStatusUpdate {
            notebook_id: "notebook-proj".to_string(),
            source_id: "s-proj".to_string(),
            status: "synced".to_string(),
            chunk_count: 1,
            projected_chunk_count: 1,
            healthy_link_count: 1,
            degraded_link_count: 0,
            last_receipt_id: Some("receipt-proj".to_string()),
            last_error: None,
            artifact_generation_id: None,
            vector_artifact_manifest_digest: None,
        })
        .unwrap();

        let summary = db
            .semantic_memory_projection_summary(
                "notebook-proj",
                &crate::retrieval::source_scope::SourceScope::All
                    .resolve(&db.list_sources().unwrap()),
            )
            .unwrap();
        assert_eq!(summary.projected_sources, 1);
        assert_eq!(summary.healthy_links, 1);
        assert!(!summary.projection_required);
    }

    #[test]
    fn projection_summary_counts_zero_chunk_sources_as_skipped() {
        let db = test_db();
        db.insert_source(&Source {
            id: "empty-source".to_string(),
            source_type: "text".to_string(),
            title: "Empty".to_string(),
            original_filename: None,
            file_hash: None,
            url: None,
            file_path: None,
            content_text: None,
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
        })
        .unwrap();

        let scope =
            crate::retrieval::source_scope::SourceScope::All.resolve(&db.list_sources().unwrap());
        let summary = db
            .semantic_memory_projection_summary("notebook-empty", &scope)
            .unwrap();

        assert_eq!(summary.total_sources, 1);
        assert_eq!(summary.zero_chunk_sources, 1);
        assert_eq!(summary.skipped_no_chunks, 1);
        assert!(!summary.projection_required);
    }

    #[test]
    fn projection_summary_is_scope_aware_for_failed_source() {
        let db = test_db();
        for id in ["healthy", "failed"] {
            db.insert_source(&Source {
                id: id.to_string(),
                source_type: "text".to_string(),
                title: id.to_string(),
                original_filename: None,
                file_hash: None,
                url: None,
                file_path: None,
                content_text: Some(format!("{id} content")),
                word_count: Some(2),
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
            db.insert_chunk(&Chunk {
                id: format!("chunk-{id}"),
                source_id: id.to_string(),
                chunk_index: 0,
                content: format!("semantic {id}"),
                token_count: Some(2),
                start_offset: None,
                end_offset: None,
                metadata: None,
                embedding_id: None,
                embedding_model: None,
            })
            .unwrap();
        }

        db.upsert_semantic_memory_projection_status(&SemanticMemoryProjectionStatusUpdate {
            notebook_id: "notebook-scope".to_string(),
            source_id: "healthy".to_string(),
            status: "synced".to_string(),
            chunk_count: 1,
            projected_chunk_count: 1,
            healthy_link_count: 1,
            degraded_link_count: 0,
            last_receipt_id: Some("receipt-healthy".to_string()),
            last_error: None,
            artifact_generation_id: None,
            vector_artifact_manifest_digest: None,
        })
        .unwrap();
        db.upsert_semantic_memory_projection_status(&SemanticMemoryProjectionStatusUpdate {
            notebook_id: "notebook-scope".to_string(),
            source_id: "failed".to_string(),
            status: "failed".to_string(),
            chunk_count: 1,
            projected_chunk_count: 0,
            healthy_link_count: 0,
            degraded_link_count: 1,
            last_receipt_id: Some("receipt-failed".to_string()),
            last_error: Some("projection failed".to_string()),
            artifact_generation_id: None,
            vector_artifact_manifest_digest: None,
        })
        .unwrap();

        let sources = db.list_sources().unwrap();
        let healthy_scope =
            crate::retrieval::source_scope::SourceScope::Explicit(vec!["healthy".to_string()])
                .resolve(&sources);
        let failed_scope =
            crate::retrieval::source_scope::SourceScope::Explicit(vec!["failed".to_string()])
                .resolve(&sources);

        let healthy_summary = db
            .semantic_memory_projection_summary("notebook-scope", &healthy_scope)
            .unwrap();
        let failed_summary = db
            .semantic_memory_projection_summary("notebook-scope", &failed_scope)
            .unwrap();

        assert_eq!(healthy_summary.failed_sources, 0);
        assert!(!healthy_summary.projection_required);
        assert_eq!(failed_summary.failed_sources, 1);
        assert!(failed_summary.projection_required);
        assert_eq!(db.get_source("failed").unwrap().status, "ready");
    }

    /// Verifies that set_selected_sources is atomic: the selection changes
    /// from one consistent state to another with no intermediate "all deselected" state.
    #[test]
    fn test_set_selected_sources_atomic() {
        let db = test_db();

        // Insert 4 sources, all initially selected
        for id in ["s1", "s2", "s3", "s4"] {
            db.insert_source(&Source {
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

        // Verify initial state: all 4 selected
        let selected = db.get_selected_source_ids().unwrap();
        assert_eq!(selected.len(), 4);

        // Change selection to only s2 and s4
        db.set_selected_sources(&["s2".to_string(), "s4".to_string()])
            .unwrap();

        // After the call, exactly s2 and s4 must be selected — no intermediate
        // state where all are deselected or only a subset of the old selection remains
        let selected = db.get_selected_source_ids().unwrap();
        assert_eq!(selected.len(), 2);
        assert!(selected.contains(&"s2".to_string()));
        assert!(selected.contains(&"s4".to_string()));

        // Also verify via list_sources that the other sources are NOT selected
        let sources = db.list_sources().unwrap();
        for source in &sources {
            if source.id == "s2" || source.id == "s4" {
                assert!(source.selected, "{} should be selected", source.id);
            } else {
                assert!(!source.selected, "{} should NOT be selected", source.id);
            }
        }

        // Test emptying the selection entirely
        db.set_selected_sources(&[]).unwrap();
        let selected = db.get_selected_source_ids().unwrap();
        assert!(
            selected.is_empty(),
            "no sources should be selected after clearing"
        );

        // Verify all sources are deselected via list_sources
        let sources = db.list_sources().unwrap();
        assert!(sources.iter().all(|s| !s.selected));

        // Select all again
        db.set_selected_sources(&[
            "s1".to_string(),
            "s2".to_string(),
            "s3".to_string(),
            "s4".to_string(),
        ])
        .unwrap();
        let selected = db.get_selected_source_ids().unwrap();
        assert_eq!(selected.len(), 4);
    }
}
