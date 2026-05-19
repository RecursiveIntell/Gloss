//! Database initialization, migrations, integrity checks, and durable sidecar state.

use crate::config::{EmbeddingConfig, MemoryLimits, PoolConfig};
use crate::error::MemoryError;
use crate::quantize::unpack_quantized;
#[cfg(feature = "turbo-quant-codec")]
use crate::types::{DerivedVectorArtifactGenerationV1, VectorArtifactBuildReceiptV1};
use crate::types::{EpisodeOutcome, Role, VectorSearchReceiptV1, VerificationStatus};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use stack_ids::ContentDigest;
#[cfg(feature = "turbo-quant-codec")]
use stack_ids::DigestBuilder;
use std::path::Path;

/// V1 migration: full schema.
const MIGRATION_V1: &str = r#"
-- CONVERSATIONS
CREATE TABLE sessions (
    id          TEXT PRIMARY KEY,
    channel     TEXT NOT NULL DEFAULT 'repl',
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    metadata    TEXT
);

CREATE INDEX idx_sessions_updated ON sessions(updated_at DESC);

CREATE TABLE messages (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role        TEXT NOT NULL CHECK (role IN ('system', 'user', 'assistant', 'tool')),
    content     TEXT NOT NULL,
    token_count INTEGER,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    metadata    TEXT
);

CREATE INDEX idx_messages_session ON messages(session_id, created_at ASC);
CREATE INDEX idx_messages_created ON messages(created_at DESC);

-- KNOWLEDGE (Facts)
CREATE TABLE facts (
    id          TEXT PRIMARY KEY,
    namespace   TEXT NOT NULL DEFAULT 'general',
    content     TEXT NOT NULL,
    source      TEXT,
    embedding   BLOB,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    metadata    TEXT
);

CREATE INDEX idx_facts_namespace ON facts(namespace);
CREATE INDEX idx_facts_updated ON facts(updated_at DESC);

CREATE TABLE facts_rowid_map (
    rowid       INTEGER PRIMARY KEY AUTOINCREMENT,
    fact_id     TEXT NOT NULL UNIQUE REFERENCES facts(id) ON DELETE CASCADE
);

CREATE VIRTUAL TABLE facts_fts USING fts5(
    content,
    content='',
    content_rowid='rowid',
    tokenize='porter unicode61'
);

-- DOCUMENTS (Chunked content)
CREATE TABLE documents (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    source_path TEXT,
    namespace   TEXT NOT NULL DEFAULT 'general',
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    metadata    TEXT
);

CREATE TABLE chunks (
    id          TEXT PRIMARY KEY,
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL,
    content     TEXT NOT NULL,
    token_count INTEGER,
    embedding   BLOB,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_chunks_document ON chunks(document_id, chunk_index ASC);

CREATE TABLE chunks_rowid_map (
    rowid       INTEGER PRIMARY KEY AUTOINCREMENT,
    chunk_id    TEXT NOT NULL UNIQUE REFERENCES chunks(id) ON DELETE CASCADE
);

CREATE VIRTUAL TABLE chunks_fts USING fts5(
    content,
    content='',
    content_rowid='rowid',
    tokenize='porter unicode61'
);

-- EMBEDDING METADATA
CREATE TABLE embedding_metadata (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    model_name  TEXT NOT NULL,
    dimensions  INTEGER NOT NULL,
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
"#;

/// V2 migration: message embeddings for conversation search.
const MIGRATION_V2: &str = r#"
ALTER TABLE messages ADD COLUMN embedding BLOB;

CREATE TABLE messages_rowid_map (
    rowid       INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id  INTEGER NOT NULL UNIQUE REFERENCES messages(id) ON DELETE CASCADE
);

CREATE VIRTUAL TABLE messages_fts USING fts5(
    content,
    content='',
    content_rowid='rowid',
    tokenize='porter unicode61'
);
"#;

/// V3 migration: embedding staleness tracking.
const MIGRATION_V3: &str = r#"
ALTER TABLE embedding_metadata ADD COLUMN embeddings_dirty INTEGER NOT NULL DEFAULT 0;
"#;

/// V4 migration: HNSW metadata tracking.
const MIGRATION_V4: &str = r#"
CREATE TABLE IF NOT EXISTS hnsw_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

/// V5 migration: quantized embeddings + HNSW keymap persistence.
const MIGRATION_V5: &str = r#"
ALTER TABLE facts ADD COLUMN embedding_q8 BLOB;
ALTER TABLE chunks ADD COLUMN embedding_q8 BLOB;
ALTER TABLE messages ADD COLUMN embedding_q8 BLOB;

CREATE TABLE IF NOT EXISTS hnsw_keymap (
    node_id     INTEGER PRIMARY KEY,
    item_key    TEXT NOT NULL UNIQUE,
    deleted     INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_hnsw_keymap_key ON hnsw_keymap(item_key);
"#;

/// V6 migration: episodes table for causal tracking.
const MIGRATION_V6: &str = r#"
CREATE TABLE IF NOT EXISTS episodes (
    document_id TEXT PRIMARY KEY REFERENCES documents(id) ON DELETE CASCADE,
    cause_ids TEXT NOT NULL,
    effect_type TEXT NOT NULL,
    outcome TEXT NOT NULL DEFAULT 'pending',
    confidence REAL NOT NULL DEFAULT 0.0,
    verification_status TEXT NOT NULL DEFAULT '{"status":"unverified"}',
    experiment_id TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_episodes_effect_type ON episodes(effect_type);
CREATE INDEX IF NOT EXISTS idx_episodes_outcome ON episodes(outcome);
CREATE INDEX IF NOT EXISTS idx_episodes_experiment_id ON episodes(experiment_id);
"#;

/// V7 migration: searchable episodes + durable sidecar journal.
const MIGRATION_V7: &str = r#"
ALTER TABLE episodes ADD COLUMN updated_at TEXT NOT NULL DEFAULT (datetime('now'));
ALTER TABLE episodes ADD COLUMN search_text TEXT NOT NULL DEFAULT '';
ALTER TABLE episodes ADD COLUMN embedding BLOB;
ALTER TABLE episodes ADD COLUMN embedding_q8 BLOB;

CREATE TABLE IF NOT EXISTS episodes_rowid_map (
    rowid       INTEGER PRIMARY KEY AUTOINCREMENT,
    document_id TEXT NOT NULL UNIQUE REFERENCES episodes(document_id) ON DELETE CASCADE
);

CREATE VIRTUAL TABLE episodes_fts USING fts5(
    content,
    content='',
    content_rowid='rowid',
    tokenize='porter unicode61'
);

CREATE TABLE IF NOT EXISTS pending_index_ops (
    item_key      TEXT PRIMARY KEY,
    entity_type   TEXT NOT NULL,
    op_kind       TEXT NOT NULL CHECK (op_kind IN ('upsert', 'delete')),
    attempt_count INTEGER NOT NULL DEFAULT 0,
    last_error    TEXT,
    updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT OR IGNORE INTO hnsw_metadata (key, value) VALUES ('sidecar_dirty', '0');

UPDATE episodes
SET search_text = TRIM(
    COALESCE(effect_type, '') || ' ' ||
    COALESCE(outcome, '') || ' ' ||
    COALESCE(experiment_id, '') || ' ' ||
    COALESCE(cause_ids, '')
)
WHERE search_text = '';

INSERT OR IGNORE INTO episodes_rowid_map (document_id)
SELECT document_id FROM episodes;

INSERT INTO episodes_fts (rowid, content)
SELECT rm.rowid, e.search_text
FROM episodes_rowid_map rm
JOIN episodes e ON e.document_id = rm.document_id;
"#;

/// V8 migration: durable episode trace IDs.
const MIGRATION_V8: &str = r#"
ALTER TABLE episodes ADD COLUMN trace_id TEXT;
"#;

/// V9 migration: first-class episode identity + normalized causal edge table.
///
/// Rebuilds the episodes table so `episode_id` is the primary key while
/// `document_id` becomes a non-unique FK allowing multiple episodes per doc.
/// Adds `episode_causes` for normalized causal backlinks.
///
/// Applied via `run_migration_v9()` because it requires table rebuild.
const MIGRATION_V9: &str = "";

/// V18 migration: durable, replay-addressable search receipts.
const MIGRATION_V18: &str = r#"
CREATE TABLE IF NOT EXISTS search_receipts (
    receipt_id             TEXT PRIMARY KEY,
    schema_version         TEXT NOT NULL,
    evaluation_time        TEXT NOT NULL,
    search_profile         TEXT NOT NULL,
    candidate_backend      TEXT NOT NULL,
    approximate            INTEGER NOT NULL CHECK (approximate IN (0, 1)),
    exact_rerank           INTEGER NOT NULL CHECK (exact_rerank IN (0, 1)),
    fallback               TEXT,
    requested_candidates   INTEGER NOT NULL CHECK (requested_candidates >= 0),
    returned_candidates    INTEGER NOT NULL CHECK (returned_candidates >= 0),
    post_filter_candidates INTEGER NOT NULL CHECK (post_filter_candidates >= 0),
    result_ids_json        TEXT NOT NULL,
    receipt_json           TEXT NOT NULL,
    receipt_digest         TEXT NOT NULL,
    created_at             TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_search_receipts_created
ON search_receipts(created_at DESC);

CREATE INDEX IF NOT EXISTS idx_search_receipts_backend
ON search_receipts(candidate_backend);
"#;

/// V19 migration: rebuildable derived vector acceleration artifacts.
const MIGRATION_V19: &str = r#"
CREATE TABLE IF NOT EXISTS derived_vector_artifacts (
    item_key                TEXT NOT NULL,
    codec_family            TEXT NOT NULL,
    codec_profile_digest    TEXT NOT NULL,
    source_embedding_digest TEXT NOT NULL,
    encoded_digest          TEXT NOT NULL,
    artifact_digest         TEXT NOT NULL,
    encoding                TEXT NOT NULL,
    dim                     INTEGER NOT NULL,
    encoded                 BLOB NOT NULL,
    created_at              TEXT NOT NULL DEFAULT (datetime('now')),
    status                  TEXT NOT NULL DEFAULT 'active',
    PRIMARY KEY (item_key, codec_family, codec_profile_digest)
);

CREATE INDEX IF NOT EXISTS idx_derived_vector_artifacts_profile
ON derived_vector_artifacts(codec_family, codec_profile_digest, status);

CREATE INDEX IF NOT EXISTS idx_derived_vector_artifacts_source_digest
ON derived_vector_artifacts(source_embedding_digest);
"#;

/// V20 migration: align derived vector artifact rows with P31 evidence fields.
const MIGRATION_V20: &str = r#"
-- Procedural migration; see run_migration_v20.
"#;

/// V21 migration: generation-level manifests for derived vector artifacts.
const MIGRATION_V21: &str = r#"
CREATE TABLE IF NOT EXISTS derived_vector_artifact_generations (
    generation_id            TEXT PRIMARY KEY,
    schema_version           TEXT NOT NULL,
    codec_family             TEXT NOT NULL,
    codec_profile_digest     TEXT NOT NULL,
    source_snapshot_digest   TEXT NOT NULL,
    source_row_count         INTEGER NOT NULL,
    artifact_count           INTEGER NOT NULL,
    source_tables_json       TEXT NOT NULL,
    dim                      INTEGER NOT NULL,
    encoding                 TEXT NOT NULL,
    created_at               TEXT NOT NULL,
    build_receipt_id         TEXT,
    artifact_manifest_digest TEXT NOT NULL,
    status                   TEXT NOT NULL CHECK (status IN ('active', 'superseded', 'invalidated', 'failed')),
    degradations_json        TEXT NOT NULL DEFAULT '[]'
);

CREATE INDEX IF NOT EXISTS idx_derived_vector_generations_profile
ON derived_vector_artifact_generations(codec_family, codec_profile_digest, status, created_at DESC);
"#;

/// Ordered list of migrations.
#[allow(deprecated)]
const MIGRATIONS: &[(u32, &str)] = &[
    (1, MIGRATION_V1),
    (2, MIGRATION_V2),
    (3, MIGRATION_V3),
    (4, MIGRATION_V4),
    (5, MIGRATION_V5),
    (6, MIGRATION_V6),
    (7, MIGRATION_V7),
    (8, MIGRATION_V8),
    (9, MIGRATION_V9),
    (10, crate::projection_import::MIGRATION_V10),
    (11, crate::projection_storage::MIGRATION_V11),
    (12, crate::projection_storage::MIGRATION_V12),
    (13, crate::projection_storage::MIGRATION_V13),
    (14, crate::projection_storage::MIGRATION_V14),
    (15, crate::projection_storage::MIGRATION_V15),
    (16, crate::projection_storage::MIGRATION_V16),
    (17, crate::projection_storage::MIGRATION_V17),
    (18, MIGRATION_V18),
    (19, MIGRATION_V19),
    (20, MIGRATION_V20),
    (21, MIGRATION_V21),
];

/// Maximum schema version this build supports.
pub const MAX_SCHEMA_VERSION: u32 = 21;

/// Procedural migration for V9: rebuild episodes table with episode_id PK.
fn run_migration_v9(conn: &Connection) -> Result<(), MemoryError> {
    // Check if episodes table exists (fresh DBs won't have it yet at V6)
    let episodes_exist: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='episodes'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| MemoryError::MigrationFailed {
            version: 9,
            reason: format!("existence check failed: {e}"),
        })?;

    if !episodes_exist {
        // No episodes table to migrate; create the target schema directly
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS episode_causes (
                 episode_id    TEXT NOT NULL,
                 cause_node_id TEXT NOT NULL,
                 ordinal       INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY (episode_id, cause_node_id)
             );
             CREATE INDEX IF NOT EXISTS idx_episode_causes_cause ON episode_causes(cause_node_id);",
        )?;
        return Ok(());
    }

    // Disable foreign keys for table rebuild
    conn.execute_batch("PRAGMA foreign_keys = OFF;")?;

    conn.execute_batch(
        "CREATE TABLE episodes_new (
             episode_id  TEXT PRIMARY KEY,
             document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
             cause_ids   TEXT NOT NULL,
             effect_type TEXT NOT NULL,
             outcome     TEXT NOT NULL DEFAULT 'pending',
             confidence  REAL NOT NULL DEFAULT 0.0,
             verification_status TEXT NOT NULL DEFAULT '{\"status\":\"unverified\"}',
             experiment_id TEXT,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
             search_text TEXT NOT NULL DEFAULT '',
             embedding   BLOB,
             embedding_q8 BLOB,
             trace_id    TEXT
         )",
    )?;

    // Migrate existing data with deterministic episode_id
    conn.execute_batch(
        "INSERT INTO episodes_new
             (episode_id, document_id, cause_ids, effect_type, outcome, confidence,
              verification_status, experiment_id, created_at, updated_at,
              search_text, embedding, embedding_q8, trace_id)
         SELECT
             document_id || '-ep0',
             document_id, cause_ids, effect_type, outcome, confidence,
             verification_status, experiment_id, created_at, updated_at,
             search_text, embedding, embedding_q8, trace_id
         FROM episodes",
    )?;

    conn.execute_batch("DROP TABLE episodes")?;
    conn.execute_batch("ALTER TABLE episodes_new RENAME TO episodes")?;

    conn.execute_batch(
        "CREATE INDEX idx_episodes_document_id ON episodes(document_id);
         CREATE INDEX idx_episodes_effect_type ON episodes(effect_type);
         CREATE INDEX idx_episodes_outcome ON episodes(outcome);
         CREATE INDEX idx_episodes_experiment_id ON episodes(experiment_id);",
    )?;

    // Rebuild episodes_rowid_map with episode_id
    conn.execute_batch(
        "DROP TABLE IF EXISTS episodes_rowid_map;
         CREATE TABLE episodes_rowid_map (
             rowid       INTEGER PRIMARY KEY AUTOINCREMENT,
             episode_id  TEXT NOT NULL UNIQUE,
             document_id TEXT
         );
         INSERT INTO episodes_rowid_map (episode_id, document_id)
         SELECT episode_id, document_id FROM episodes;",
    )?;

    // Rebuild episodes FTS
    conn.execute_batch(
        "DROP TABLE IF EXISTS episodes_fts;
         CREATE VIRTUAL TABLE episodes_fts USING fts5(
             content,
             content='',
             content_rowid='rowid',
             tokenize='porter unicode61'
         );
         INSERT INTO episodes_fts (rowid, content)
         SELECT rm.rowid, e.search_text
         FROM episodes_rowid_map rm
         JOIN episodes e ON e.episode_id = rm.episode_id;",
    )?;

    // Normalized causal edge table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS episode_causes (
             episode_id    TEXT NOT NULL,
             cause_node_id TEXT NOT NULL,
             ordinal       INTEGER NOT NULL DEFAULT 0,
             PRIMARY KEY (episode_id, cause_node_id)
         );
         CREATE INDEX IF NOT EXISTS idx_episode_causes_cause ON episode_causes(cause_node_id);",
    )?;

    // Populate edge table from existing JSON cause_ids
    conn.execute_batch(
        "INSERT OR IGNORE INTO episode_causes (episode_id, cause_node_id, ordinal)
         SELECT e.episode_id, je.value, CAST(je.key AS INTEGER)
         FROM episodes e, json_each(e.cause_ids) je;",
    )?;

    conn.execute_batch("PRAGMA foreign_keys = ON;")?;

    Ok(())
}

/// How thorough the integrity check should be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyMode {
    /// Quick: counts and basic metadata only.
    Quick,
    /// Full: includes FTS, JSON/enum decoding, blobs, and SQLite integrity_check.
    Full,
}

/// Result of an integrity verification.
#[derive(Debug, Clone)]
pub struct IntegrityReport {
    pub ok: bool,
    pub schema_version: u32,
    pub fact_count: usize,
    pub chunk_count: usize,
    pub message_count: usize,
    pub facts_missing_embeddings: usize,
    pub chunks_missing_embeddings: usize,
    pub issues: Vec<String>,
}

/// Action to take when integrity issues are found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileAction {
    ReportOnly,
    RebuildFts,
    ReEmbed,
}

/// Desired HNSW sidecar mutation queued in SQLite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexOpKind {
    Upsert,
    Delete,
}

impl IndexOpKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Upsert => "upsert",
            Self::Delete => "delete",
        }
    }

    fn parse(raw: &str, item_key: &str) -> Result<Self, MemoryError> {
        match raw {
            "upsert" => Ok(Self::Upsert),
            "delete" => Ok(Self::Delete),
            other => Err(MemoryError::CorruptData {
                table: "pending_index_ops",
                row_id: item_key.to_string(),
                detail: format!("invalid op_kind '{other}'"),
            }),
        }
    }
}

/// Durable sidecar repair record.
#[derive(Debug, Clone)]
pub(crate) struct PendingIndexOp {
    pub item_key: String,
    pub entity_type: String,
    pub op_kind: IndexOpKind,
    pub attempt_count: u32,
    pub last_error: Option<String>,
}

/// Run a closure inside an unchecked transaction, committing on success.
pub fn with_transaction<F, T>(conn: &Connection, f: F) -> Result<T, MemoryError>
where
    F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, MemoryError>,
{
    let tx = conn.unchecked_transaction()?;
    let result = f(&tx)?;
    tx.commit()?;
    Ok(result)
}

/// Open or create a SQLite database, configure pragmas, and run migrations.
pub fn open_database(
    path: &Path,
    pool: &PoolConfig,
    limits: &MemoryLimits,
) -> Result<Connection, MemoryError> {
    open_database_internal(path, pool, limits.max_db_size_bytes, true)
}

/// Open a SQLite connection with pragmas applied but without running migrations.
pub fn open_database_connection(
    path: &Path,
    pool: &PoolConfig,
    limits: &MemoryLimits,
) -> Result<Connection, MemoryError> {
    open_database_internal(path, pool, limits.max_db_size_bytes, false)
}

pub(crate) fn open_database_internal(
    path: &Path,
    pool: &PoolConfig,
    max_db_size_bytes: u64,
    run_schema_migrations: bool,
) -> Result<Connection, MemoryError> {
    create_parent_dirs(path)?;
    let conn = Connection::open(path)?;
    configure_connection(&conn, path, pool, max_db_size_bytes, false)?;
    if run_schema_migrations {
        run_migrations(&conn)?;
    }
    Ok(conn)
}

pub(crate) fn open_pool_member_connection(
    path: &Path,
    pool: &PoolConfig,
    limits: &MemoryLimits,
    query_only: bool,
) -> Result<Connection, MemoryError> {
    create_parent_dirs(path)?;
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
    let conn = Connection::open_with_flags(path, flags)?;
    configure_connection(&conn, path, pool, limits.max_db_size_bytes, query_only)?;
    Ok(conn)
}

fn create_parent_dirs(path: &Path) -> Result<(), MemoryError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                MemoryError::StorageError(format!(
                    "failed to create database directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }
    }
    Ok(())
}

fn configure_connection(
    conn: &Connection,
    path: &Path,
    pool: &PoolConfig,
    max_db_size_bytes: u64,
    query_only: bool,
) -> Result<(), MemoryError> {
    let journal_mode = if pool.enable_wal { "WAL" } else { "DELETE" };
    conn.execute_batch(&format!(
        "PRAGMA journal_mode = {};
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = {};
         PRAGMA synchronous = NORMAL;
         PRAGMA temp_store = MEMORY;
         PRAGMA wal_autocheckpoint = {};",
        journal_mode, pool.busy_timeout_ms, pool.wal_autocheckpoint,
    ))?;

    if query_only {
        conn.execute_batch("PRAGMA query_only = ON;")?;
    }

    let actual_journal_mode: String =
        conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    let expected_journal_mode = if pool.enable_wal { "wal" } else { "delete" };
    if actual_journal_mode.to_lowercase() != expected_journal_mode {
        return Err(MemoryError::StorageError(format!(
            "SQLite journal mode mismatch for {}: requested {}, got {}",
            path.display(),
            expected_journal_mode,
            actual_journal_mode
        )));
    }

    if max_db_size_bytes > 0 {
        let page_size: u64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
        let max_page_count = max_db_size_bytes.div_ceil(page_size);
        let actual_max_page_count: u64 = conn.query_row(
            &format!("PRAGMA max_page_count = {}", max_page_count),
            [],
            |row| row.get(0),
        )?;
        let page_count: u64 = conn.query_row("PRAGMA page_count", [], |row| row.get(0))?;

        if page_count > actual_max_page_count {
            return Err(MemoryError::DatabaseSizeLimitExceeded {
                current: page_count.saturating_mul(page_size),
                limit: max_db_size_bytes,
            });
        }
    }

    Ok(())
}

/// Run all pending migrations.
pub fn run_migrations(conn: &Connection) -> Result<(), MemoryError> {
    let user_version: u32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| MemoryError::MigrationFailed {
            version: 0,
            reason: format!("failed to read PRAGMA user_version: {e}"),
        })?;

    if user_version > MAX_SCHEMA_VERSION {
        return Err(MemoryError::SchemaAhead {
            found: user_version,
            supported: MAX_SCHEMA_VERSION,
        });
    }

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _schema_version (
            version     INTEGER PRIMARY KEY,
            applied_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;

    for &(version, sql) in MIGRATIONS {
        let current_version: u32 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM _schema_version",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if current_version >= version {
            continue;
        }

        with_transaction(conn, |tx| {
            match version {
                9 => run_migration_v9(tx).map_err(|e| MemoryError::MigrationFailed {
                    version,
                    reason: e.to_string(),
                })?,
                16 => run_migration_v16(tx).map_err(|e| MemoryError::MigrationFailed {
                    version,
                    reason: e.to_string(),
                })?,
                17 => run_migration_v17(tx).map_err(|e| MemoryError::MigrationFailed {
                    version,
                    reason: e.to_string(),
                })?,
                20 => run_migration_v20(tx).map_err(|e| MemoryError::MigrationFailed {
                    version,
                    reason: e.to_string(),
                })?,
                21 => run_migration_v21(tx).map_err(|e| MemoryError::MigrationFailed {
                    version,
                    reason: e.to_string(),
                })?,
                _ => tx
                    .execute_batch(sql)
                    .map_err(|e| MemoryError::MigrationFailed {
                        version,
                        reason: e.to_string(),
                    })?,
            }
            tx.execute(
                "INSERT INTO _schema_version (version) VALUES (?1)",
                params![version],
            )
            .map_err(|e| MemoryError::MigrationFailed {
                version,
                reason: e.to_string(),
            })?;
            Ok(())
        })?;

        tracing::info!("Applied migration V{}", version);
    }

    let final_version: u32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM _schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    conn.execute_batch(&format!("PRAGMA user_version = {};", final_version))?;

    Ok(())
}

fn run_migration_v16(conn: &Connection) -> Result<(), rusqlite::Error> {
    add_column_if_missing(conn, "projection_import_log", "kernel_payload_json", "TEXT")?;
    add_column_if_missing(
        conn,
        "projection_import_failures",
        "kernel_payload_json",
        "TEXT",
    )?;
    Ok(())
}

fn run_migration_v17(conn: &Connection) -> Result<(), rusqlite::Error> {
    add_column_if_missing(conn, "projection_import_log", "episode_bundle_id", "TEXT")?;
    add_column_if_missing(conn, "projection_import_log", "episode_bundle_json", "TEXT")?;
    add_column_if_missing(
        conn,
        "projection_import_log",
        "execution_context_json",
        "TEXT",
    )?;
    add_column_if_missing(
        conn,
        "projection_import_failures",
        "episode_bundle_id",
        "TEXT",
    )?;
    add_column_if_missing(
        conn,
        "projection_import_failures",
        "episode_bundle_json",
        "TEXT",
    )?;
    add_column_if_missing(
        conn,
        "projection_import_failures",
        "execution_context_json",
        "TEXT",
    )?;
    Ok(())
}

fn run_migration_v20(conn: &Connection) -> Result<(), rusqlite::Error> {
    add_column_if_missing(conn, "derived_vector_artifacts", "encoded_digest", "TEXT")?;
    conn.execute(
        "UPDATE derived_vector_artifacts
         SET encoded_digest = artifact_digest
         WHERE encoded_digest IS NULL OR encoded_digest = ''",
        [],
    )?;
    add_column_if_missing(
        conn,
        "derived_vector_artifacts",
        "encoding",
        "TEXT NOT NULL DEFAULT 'turbo_code_wire_v1'",
    )?;
    add_column_if_missing(
        conn,
        "derived_vector_artifacts",
        "dim",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(
        conn,
        "derived_vector_artifacts",
        "status",
        "TEXT NOT NULL DEFAULT 'active'",
    )?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_derived_vector_artifacts_profile
         ON derived_vector_artifacts(codec_family, codec_profile_digest, status);
         CREATE INDEX IF NOT EXISTS idx_derived_vector_artifacts_source_digest
         ON derived_vector_artifacts(source_embedding_digest);",
    )?;
    Ok(())
}

fn run_migration_v21(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(MIGRATION_V21)?;
    add_column_if_missing(conn, "derived_vector_artifacts", "generation_id", "TEXT")?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_derived_vector_artifacts_generation
         ON derived_vector_artifacts(generation_id, status);",
    )?;
    Ok(())
}

const SEARCH_RECEIPT_SCHEMA_VERSION: &str = "vector_search_receipt_v1";

#[derive(Debug, Serialize, Deserialize)]
struct StoredVectorSearchReceiptV1 {
    #[serde(default = "default_search_receipt_schema_version")]
    schema_version: String,
    receipt_id: String,
    evaluation_time: DateTime<Utc>,
    #[serde(default)]
    receipt_digest: Option<String>,
    #[serde(default)]
    trace_id: Option<String>,
    #[serde(default)]
    attempt_family_id: Option<String>,
    #[serde(default)]
    attempt_id: Option<String>,
    #[serde(default)]
    replay_of: Option<String>,
    query_embedding_digest: Option<String>,
    #[serde(default)]
    query_text_digest: Option<String>,
    #[serde(default)]
    query_input_digest: Option<String>,
    #[serde(default)]
    filter_digest: Option<String>,
    #[serde(default)]
    redaction_state: Option<String>,
    #[serde(default)]
    budget_id: Option<String>,
    #[serde(default)]
    deadline_at: Option<DateTime<Utc>>,
    search_profile: String,
    candidate_backend: String,
    codec_family: Option<String>,
    codec_profile_digest: Option<String>,
    #[serde(default)]
    artifact_profile_digest: Option<String>,
    #[serde(default)]
    artifact_count: Option<u64>,
    #[serde(default)]
    artifact_corruption_count: Option<u64>,
    #[serde(default)]
    artifact_missing_count: Option<u64>,
    #[serde(default)]
    vector_artifact_manifest_digest: Option<String>,
    #[serde(default)]
    artifact_generation_id: Option<String>,
    #[serde(default)]
    approximate_scanned_count: Option<u64>,
    #[serde(default)]
    approximate_returned_count: Option<u64>,
    #[serde(default)]
    raw_rows_loaded_count: Option<u64>,
    #[serde(default)]
    filter_strategy: Option<String>,
    #[serde(default)]
    vector_artifact_count: Option<u64>,
    #[serde(default)]
    vector_artifact_missing_count: Option<u64>,
    #[serde(default)]
    vector_artifact_stale_count: Option<u64>,
    #[serde(default)]
    exact_rerank_count: Option<u64>,
    #[serde(default)]
    approximate_candidate_count: Option<u64>,
    #[serde(default)]
    fallback_reason: Option<String>,
    approximate: bool,
    requested_candidates: u64,
    returned_candidates: u64,
    post_filter_candidates: u64,
    fallback: Option<String>,
    exact_rerank: bool,
    result_ids: Vec<String>,
    degradations: Vec<String>,
}

fn default_search_receipt_schema_version() -> String {
    SEARCH_RECEIPT_SCHEMA_VERSION.to_string()
}

fn b3_digest(bytes: &[u8]) -> String {
    format!("blake3:{}", ContentDigest::compute(bytes).hex())
}

/// Row from the derived vector artifact store.
#[cfg(feature = "turbo-quant-codec")]
#[derive(Debug, Clone)]
pub(crate) struct DerivedVectorArtifactRow {
    pub item_key: String,
    pub generation_id: Option<String>,
    pub codec_family: String,
    pub codec_profile_digest: String,
    pub source_embedding_digest: String,
    pub encoded_digest: String,
    pub encoding: String,
    pub dim: usize,
    pub status: String,
    pub encoded: Vec<u8>,
}

/// Active derived vector artifact generation row.
#[cfg(feature = "turbo-quant-codec")]
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct DerivedVectorArtifactGenerationRow {
    pub generation_id: String,
    pub codec_family: String,
    pub codec_profile_digest: String,
    pub source_snapshot_digest: String,
    pub source_row_count: usize,
    pub artifact_count: usize,
    pub dim: usize,
    pub encoding: String,
    pub artifact_manifest_digest: String,
    pub status: String,
}

/// Stable digest for an authoritative raw f32 embedding BLOB.
#[cfg(feature = "turbo-quant-codec")]
pub(crate) fn source_embedding_digest(
    blob: &[u8],
    expected_dim: usize,
) -> Result<String, MemoryError> {
    validate_vector_blob_len(blob, expected_dim)?;
    let mut builder = DigestBuilder::new();
    builder
        .update_str("semantic-memory.source_embedding.v1")
        .separator()
        .update(&(expected_dim as u64).to_le_bytes())
        .separator()
        .update(blob);
    Ok(format!("blake3:{}", builder.finalize().hex()))
}

#[cfg(feature = "turbo-quant-codec")]
fn source_snapshot_digest(rows: &[DerivedVectorArtifactRow], dim: usize) -> String {
    let mut entries = rows
        .iter()
        .map(|row| (row.item_key.as_str(), row.source_embedding_digest.as_str()))
        .collect::<Vec<_>>();
    entries.sort_unstable();

    let mut builder = DigestBuilder::new();
    builder
        .update_str("semantic-memory.vector_source_snapshot.v1")
        .separator()
        .update(&(dim as u64).to_le_bytes())
        .separator();
    for (item_key, source_embedding_digest) in entries {
        builder
            .update_str(item_key)
            .separator()
            .update_str(source_embedding_digest)
            .separator();
    }
    format!("blake3:{}", builder.finalize().hex())
}

#[cfg(feature = "turbo-quant-codec")]
pub(crate) fn current_source_snapshot_digest(
    conn: &Connection,
    dim: usize,
) -> Result<(String, usize), MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT 'fact:' || id AS item_key, embedding FROM facts WHERE embedding IS NOT NULL
         UNION ALL
         SELECT 'chunk:' || id AS item_key, embedding FROM chunks WHERE embedding IS NOT NULL
         UNION ALL
         SELECT 'msg:' || id AS item_key, embedding FROM messages WHERE embedding IS NOT NULL
         UNION ALL
         SELECT 'episode:' || episode_id AS item_key, embedding FROM episodes WHERE embedding IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
    })?;

    let mut entries = Vec::new();
    for row in rows {
        let (item_key, blob) = row?;
        entries.push((item_key, source_embedding_digest(&blob, dim)?));
    }
    entries.sort_unstable();

    let mut builder = DigestBuilder::new();
    builder
        .update_str("semantic-memory.vector_source_snapshot.v1")
        .separator()
        .update(&(dim as u64).to_le_bytes())
        .separator();
    for (item_key, source_embedding_digest) in &entries {
        builder
            .update_str(item_key)
            .separator()
            .update_str(source_embedding_digest)
            .separator();
    }
    Ok((
        format!("blake3:{}", builder.finalize().hex()),
        entries.len(),
    ))
}

#[cfg(feature = "turbo-quant-codec")]
fn derived_artifact_manifest_digest(rows: &[DerivedVectorArtifactRow]) -> String {
    let mut entries = rows
        .iter()
        .map(|row| {
            (
                row.item_key.as_str(),
                row.source_embedding_digest.as_str(),
                row.encoded_digest.as_str(),
            )
        })
        .collect::<Vec<_>>();
    entries.sort_unstable();

    let mut builder = DigestBuilder::new();
    builder
        .update_str("semantic-memory.vector_artifact_manifest.v1")
        .separator();
    for (item_key, source_embedding_digest, encoded_digest) in entries {
        builder
            .update_str(item_key)
            .separator()
            .update_str(source_embedding_digest)
            .separator()
            .update_str(encoded_digest)
            .separator();
    }
    format!("blake3:{}", builder.finalize().hex())
}

#[cfg(feature = "turbo-quant-codec")]
pub(crate) fn upsert_derived_vector_artifact(
    conn: &Connection,
    row: &DerivedVectorArtifactRow,
) -> Result<(), MemoryError> {
    conn.execute(
        "INSERT OR REPLACE INTO derived_vector_artifacts
             (item_key, generation_id, codec_family, codec_profile_digest, source_embedding_digest,
              encoded_digest, artifact_digest, encoding, dim, encoded, created_at, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8, ?9, datetime('now'), ?10)",
        params![
            row.item_key,
            row.generation_id.as_deref(),
            row.codec_family,
            row.codec_profile_digest,
            row.source_embedding_digest,
            row.encoded_digest,
            row.encoding,
            i64::try_from(row.dim)
                .map_err(|err| MemoryError::Other(format!("artifact dim overflow: {err}")))?,
            row.encoded,
            row.status
        ],
    )?;
    Ok(())
}

pub fn delete_derived_vector_artifact(
    conn: &Connection,
    item_key: &str,
) -> Result<(), MemoryError> {
    conn.execute(
        "DELETE FROM derived_vector_artifacts WHERE item_key = ?1",
        params![item_key],
    )?;
    Ok(())
}

pub fn invalidate_derived_vector_artifact(
    conn: &Connection,
    item_key: &str,
) -> Result<(), MemoryError> {
    conn.execute(
        "UPDATE derived_vector_artifacts
         SET status = 'invalidated'
         WHERE item_key = ?1 AND status = 'active'",
        params![item_key],
    )?;
    conn.execute(
        "UPDATE derived_vector_artifact_generations
         SET status = 'invalidated'
         WHERE status = 'active'",
        [],
    )?;
    Ok(())
}

#[cfg(feature = "turbo-quant-codec")]
#[allow(dead_code)]
pub(crate) fn load_derived_vector_artifacts_by_profile(
    conn: &Connection,
    codec_family: &str,
    codec_profile_digest: &str,
) -> Result<Vec<DerivedVectorArtifactRow>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT item_key, generation_id, codec_family, codec_profile_digest, source_embedding_digest,
                encoded_digest, encoding, dim, status, encoded
         FROM derived_vector_artifacts
         WHERE codec_family = ?1 AND codec_profile_digest = ?2 AND status = 'active'",
    )?;
    let rows = stmt.query_map(params![codec_family, codec_profile_digest], |row| {
        let dim_i64: i64 = row.get(7)?;
        Ok(DerivedVectorArtifactRow {
            item_key: row.get(0)?,
            generation_id: row.get(1)?,
            codec_family: row.get(2)?,
            codec_profile_digest: row.get(3)?,
            source_embedding_digest: row.get(4)?,
            encoded_digest: row.get(5)?,
            encoding: row.get(6)?,
            dim: usize::try_from(dim_i64).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    7,
                    rusqlite::types::Type::Integer,
                    Box::new(err),
                )
            })?,
            status: row.get(8)?,
            encoded: row.get(9)?,
        })
    })?;

    let mut artifacts = Vec::new();
    for row in rows {
        artifacts.push(row?);
    }
    Ok(artifacts)
}

#[cfg(feature = "turbo-quant-codec")]
pub(crate) fn load_derived_vector_artifacts_by_generation(
    conn: &Connection,
    generation_id: &str,
) -> Result<Vec<DerivedVectorArtifactRow>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT item_key, generation_id, codec_family, codec_profile_digest, source_embedding_digest,
                encoded_digest, encoding, dim, status, encoded
         FROM derived_vector_artifacts
         WHERE generation_id = ?1 AND status = 'active'",
    )?;
    let rows = stmt.query_map(params![generation_id], |row| {
        let dim_i64: i64 = row.get(7)?;
        Ok(DerivedVectorArtifactRow {
            item_key: row.get(0)?,
            generation_id: row.get(1)?,
            codec_family: row.get(2)?,
            codec_profile_digest: row.get(3)?,
            source_embedding_digest: row.get(4)?,
            encoded_digest: row.get(5)?,
            encoding: row.get(6)?,
            dim: usize::try_from(dim_i64).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    7,
                    rusqlite::types::Type::Integer,
                    Box::new(err),
                )
            })?,
            status: row.get(8)?,
            encoded: row.get(9)?,
        })
    })?;

    let mut artifacts = Vec::new();
    for row in rows {
        artifacts.push(row?);
    }
    Ok(artifacts)
}

#[cfg(feature = "turbo-quant-codec")]
pub(crate) fn current_derived_vector_generation(
    conn: &Connection,
    codec_family: &str,
    codec_profile_digest: &str,
) -> Result<Option<DerivedVectorArtifactGenerationRow>, MemoryError> {
    conn.query_row(
        "SELECT generation_id, codec_family, codec_profile_digest, source_snapshot_digest,
                source_row_count, artifact_count, dim, encoding, artifact_manifest_digest, status
         FROM derived_vector_artifact_generations
         WHERE codec_family = ?1 AND codec_profile_digest = ?2 AND status = 'active'
         ORDER BY created_at DESC
         LIMIT 1",
        params![codec_family, codec_profile_digest],
        |row| {
            let source_row_count: i64 = row.get(4)?;
            let artifact_count: i64 = row.get(5)?;
            let dim: i64 = row.get(6)?;
            Ok(DerivedVectorArtifactGenerationRow {
                generation_id: row.get(0)?,
                codec_family: row.get(1)?,
                codec_profile_digest: row.get(2)?,
                source_snapshot_digest: row.get(3)?,
                source_row_count: usize::try_from(source_row_count).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Integer,
                        Box::new(err),
                    )
                })?,
                artifact_count: usize::try_from(artifact_count).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Integer,
                        Box::new(err),
                    )
                })?,
                dim: usize::try_from(dim).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Integer,
                        Box::new(err),
                    )
                })?,
                encoding: row.get(7)?,
                artifact_manifest_digest: row.get(8)?,
                status: row.get(9)?,
            })
        },
    )
    .optional()
    .map_err(MemoryError::from)
}

pub fn count_derived_vector_artifacts(
    conn: &Connection,
    codec_family: &str,
    codec_profile_digest: &str,
) -> Result<usize, MemoryError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM derived_vector_artifacts
         WHERE codec_family = ?1 AND codec_profile_digest = ?2 AND status = 'active'",
        params![codec_family, codec_profile_digest],
        |row| row.get(0),
    )?;
    usize::try_from(count)
        .map_err(|err| MemoryError::Other(format!("derived artifact count overflow: {err}")))
}

#[cfg(feature = "turbo-quant-codec")]
pub(crate) fn rebuild_turbo_quant_artifacts(
    conn: &Connection,
    dim: usize,
    bits: u8,
    projections: usize,
    seed: u64,
) -> Result<VectorArtifactBuildReceiptV1, MemoryError> {
    use crate::vector_codec::{TurboQuantCodec, VectorCodec};

    let started = std::time::Instant::now();
    let codec = TurboQuantCodec::new(dim, bits, projections, seed)?;
    let codec_profile_digest = codec.profile().digest();
    let generation_id = uuid::Uuid::new_v4().to_string();
    let mut source_row_count = 0usize;
    let mut artifact_count = 0usize;
    let mut skipped_row_count = 0usize;
    let mut degradations = Vec::new();

    let mut stmt = conn.prepare(
        "SELECT 'fact:' || id AS item_key, embedding FROM facts WHERE embedding IS NOT NULL
         UNION ALL
         SELECT 'chunk:' || id AS item_key, embedding FROM chunks WHERE embedding IS NOT NULL
         UNION ALL
         SELECT 'msg:' || id AS item_key, embedding FROM messages WHERE embedding IS NOT NULL
         UNION ALL
         SELECT 'episode:' || episode_id AS item_key, embedding FROM episodes WHERE embedding IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
    })?;

    let mut pending = Vec::new();
    for row in rows {
        let (item_key, blob) = row?;
        source_row_count += 1;
        let embedding = match decode_f32_le(&blob, dim) {
            Ok(embedding) => embedding,
            Err(err) => {
                skipped_row_count += 1;
                degradations.push(format!(
                    "skipped {item_key}: invalid authoritative embedding: {err}"
                ));
                continue;
            }
        };
        let artifact = match codec.encode(&embedding) {
            Ok(artifact) => artifact,
            Err(err) => {
                skipped_row_count += 1;
                degradations.push(format!("skipped {item_key}: encode failed: {err}"));
                continue;
            }
        };
        pending.push(DerivedVectorArtifactRow {
            item_key,
            generation_id: Some(generation_id.clone()),
            codec_family: "turbo_quant".to_string(),
            codec_profile_digest: codec_profile_digest.clone(),
            source_embedding_digest: source_embedding_digest(&blob, dim)?,
            encoded_digest: artifact.artifact_digest,
            encoding: "turbo_code_wire_v1".to_string(),
            dim,
            status: "active".to_string(),
            encoded: artifact.encoded,
        });
    }
    drop(stmt);

    let source_snapshot_digest = source_snapshot_digest(&pending, dim);
    let artifact_manifest_digest = derived_artifact_manifest_digest(&pending);
    let source_tables = vec![
        "facts".to_string(),
        "chunks".to_string(),
        "messages".to_string(),
        "episodes".to_string(),
    ];
    let generation_manifest = DerivedVectorArtifactGenerationV1 {
        schema_version: "derived_vector_artifact_generation_v1".to_string(),
        generation_id: generation_id.clone(),
        codec_family: "turbo_quant".to_string(),
        codec_profile_digest: codec_profile_digest.clone(),
        source_snapshot_digest: source_snapshot_digest.clone(),
        source_row_count,
        artifact_count: pending.len(),
        source_tables,
        dim,
        encoding: "turbo_code_wire_v1".to_string(),
        created_at: Utc::now(),
        build_receipt_id: None,
        artifact_manifest_digest: artifact_manifest_digest.clone(),
        status: if skipped_row_count == 0 {
            "active".to_string()
        } else {
            "failed".to_string()
        },
        degradations: degradations.clone(),
    };

    with_transaction(conn, |tx| {
        tx.execute(
            "UPDATE derived_vector_artifact_generations
             SET status = 'superseded'
             WHERE codec_family = ?1 AND codec_profile_digest = ?2 AND status = 'active'",
            params!["turbo_quant", &codec_profile_digest],
        )?;
        tx.execute(
            "DELETE FROM derived_vector_artifacts
             WHERE codec_family = ?1 AND codec_profile_digest = ?2",
            params!["turbo_quant", &codec_profile_digest],
        )?;
        tx.execute(
            "INSERT INTO derived_vector_artifact_generations
                (generation_id, schema_version, codec_family, codec_profile_digest,
                 source_snapshot_digest, source_row_count, artifact_count, source_tables_json,
                 dim, encoding, created_at, build_receipt_id, artifact_manifest_digest,
                 status, degradations_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                generation_manifest.generation_id,
                generation_manifest.schema_version,
                generation_manifest.codec_family,
                generation_manifest.codec_profile_digest,
                generation_manifest.source_snapshot_digest,
                i64::try_from(generation_manifest.source_row_count).map_err(|err| {
                    MemoryError::Other(format!("source row count overflow: {err}"))
                })?,
                i64::try_from(generation_manifest.artifact_count).map_err(|err| {
                    MemoryError::Other(format!("artifact count overflow: {err}"))
                })?,
                serde_json::to_string(&generation_manifest.source_tables)
                    .map_err(|err| MemoryError::Other(err.to_string()))?,
                i64::try_from(generation_manifest.dim)
                    .map_err(|err| MemoryError::Other(format!("artifact dim overflow: {err}")))?,
                generation_manifest.encoding,
                generation_manifest.created_at.to_rfc3339(),
                generation_manifest.build_receipt_id,
                generation_manifest.artifact_manifest_digest,
                generation_manifest.status,
                serde_json::to_string(&generation_manifest.degradations)
                    .map_err(|err| MemoryError::Other(err.to_string()))?,
            ],
        )?;
        for row in &pending {
            upsert_derived_vector_artifact(tx, row)?;
            artifact_count += 1;
        }
        Ok(())
    })?;

    Ok(VectorArtifactBuildReceiptV1 {
        schema_version: "vector_artifact_build_receipt_v1".to_string(),
        codec_family: "turbo_quant".to_string(),
        codec_profile_digest,
        source_row_count,
        artifact_count,
        generation_id: Some(generation_id),
        source_snapshot_digest: Some(source_snapshot_digest),
        artifact_manifest_digest: Some(artifact_manifest_digest),
        skipped_row_count,
        elapsed_ms: started.elapsed().as_millis(),
        created_at: Utc::now(),
        degradations,
    })
}

fn receipt_count_to_u64(value: usize, field: &'static str) -> Result<u64, MemoryError> {
    u64::try_from(value).map_err(|err| MemoryError::Other(format!("{field} is too large: {err}")))
}

fn receipt_count_to_i64(value: u64, field: &'static str) -> Result<i64, MemoryError> {
    i64::try_from(value).map_err(|err| MemoryError::Other(format!("{field} is too large: {err}")))
}

fn receipt_count_to_usize(
    value: u64,
    receipt_id: &str,
    field: &'static str,
) -> Result<usize, MemoryError> {
    usize::try_from(value).map_err(|err| MemoryError::CorruptData {
        table: "search_receipts",
        row_id: receipt_id.to_string(),
        detail: format!("{field} does not fit this platform: {err}"),
    })
}

fn stored_search_receipt(
    receipt: &VectorSearchReceiptV1,
) -> Result<StoredVectorSearchReceiptV1, MemoryError> {
    Ok(StoredVectorSearchReceiptV1 {
        schema_version: SEARCH_RECEIPT_SCHEMA_VERSION.to_string(),
        receipt_id: receipt.receipt_id.clone(),
        evaluation_time: receipt.evaluation_time,
        receipt_digest: receipt.receipt_digest.clone(),
        trace_id: receipt.trace_id.clone(),
        attempt_family_id: receipt.attempt_family_id.clone(),
        attempt_id: receipt.attempt_id.clone(),
        replay_of: receipt.replay_of.clone(),
        query_embedding_digest: receipt.query_embedding_digest.clone(),
        query_text_digest: receipt.query_text_digest.clone(),
        query_input_digest: receipt.query_input_digest.clone(),
        filter_digest: receipt.filter_digest.clone(),
        redaction_state: receipt.redaction_state.clone(),
        budget_id: receipt.budget_id.clone(),
        deadline_at: receipt.deadline_at,
        search_profile: receipt.search_profile.clone(),
        candidate_backend: receipt.candidate_backend.clone(),
        codec_family: receipt.codec_family.clone(),
        codec_profile_digest: receipt.codec_profile_digest.clone(),
        artifact_profile_digest: receipt.artifact_profile_digest.clone(),
        artifact_count: receipt
            .artifact_count
            .map(|value| receipt_count_to_u64(value, "artifact_count"))
            .transpose()?,
        artifact_corruption_count: receipt
            .artifact_corruption_count
            .map(|value| receipt_count_to_u64(value, "artifact_corruption_count"))
            .transpose()?,
        artifact_missing_count: receipt
            .artifact_missing_count
            .map(|value| receipt_count_to_u64(value, "artifact_missing_count"))
            .transpose()?,
        vector_artifact_manifest_digest: receipt.vector_artifact_manifest_digest.clone(),
        artifact_generation_id: receipt.artifact_generation_id.clone(),
        approximate_scanned_count: receipt
            .approximate_scanned_count
            .map(|value| receipt_count_to_u64(value, "approximate_scanned_count"))
            .transpose()?,
        approximate_returned_count: receipt
            .approximate_returned_count
            .map(|value| receipt_count_to_u64(value, "approximate_returned_count"))
            .transpose()?,
        raw_rows_loaded_count: receipt
            .raw_rows_loaded_count
            .map(|value| receipt_count_to_u64(value, "raw_rows_loaded_count"))
            .transpose()?,
        filter_strategy: receipt.filter_strategy.clone(),
        vector_artifact_count: receipt
            .vector_artifact_count
            .map(|value| receipt_count_to_u64(value, "vector_artifact_count"))
            .transpose()?,
        vector_artifact_missing_count: receipt
            .vector_artifact_missing_count
            .map(|value| receipt_count_to_u64(value, "vector_artifact_missing_count"))
            .transpose()?,
        vector_artifact_stale_count: receipt
            .vector_artifact_stale_count
            .map(|value| receipt_count_to_u64(value, "vector_artifact_stale_count"))
            .transpose()?,
        exact_rerank_count: receipt
            .exact_rerank_count
            .map(|value| receipt_count_to_u64(value, "exact_rerank_count"))
            .transpose()?,
        approximate_candidate_count: receipt
            .approximate_candidate_count
            .map(|value| receipt_count_to_u64(value, "approximate_candidate_count"))
            .transpose()?,
        fallback_reason: receipt.fallback_reason.clone(),
        approximate: receipt.approximate,
        requested_candidates: receipt_count_to_u64(
            receipt.requested_candidates,
            "requested_candidates",
        )?,
        returned_candidates: receipt_count_to_u64(
            receipt.returned_candidates,
            "returned_candidates",
        )?,
        post_filter_candidates: receipt_count_to_u64(
            receipt.post_filter_candidates,
            "post_filter_candidates",
        )?,
        fallback: receipt.fallback.clone(),
        exact_rerank: receipt.exact_rerank,
        result_ids: receipt.result_ids.clone(),
        degradations: receipt.degradations.clone(),
    })
}

fn search_receipt_from_stored(
    stored: StoredVectorSearchReceiptV1,
) -> Result<VectorSearchReceiptV1, MemoryError> {
    if stored.schema_version != SEARCH_RECEIPT_SCHEMA_VERSION {
        return Err(MemoryError::CorruptData {
            table: "search_receipts",
            row_id: stored.receipt_id,
            detail: format!(
                "unsupported receipt schema version '{}'",
                stored.schema_version
            ),
        });
    }

    Ok(VectorSearchReceiptV1 {
        schema_version: stored.schema_version.clone(),
        receipt_digest: stored.receipt_digest,
        receipt_id: stored.receipt_id.clone(),
        evaluation_time: stored.evaluation_time,
        trace_id: stored.trace_id,
        attempt_family_id: stored.attempt_family_id,
        attempt_id: stored.attempt_id,
        replay_of: stored.replay_of,
        query_embedding_digest: stored.query_embedding_digest,
        query_text_digest: stored.query_text_digest,
        query_input_digest: stored.query_input_digest,
        filter_digest: stored.filter_digest,
        redaction_state: stored.redaction_state,
        budget_id: stored.budget_id,
        deadline_at: stored.deadline_at,
        search_profile: stored.search_profile,
        candidate_backend: stored.candidate_backend,
        codec_family: stored.codec_family,
        codec_profile_digest: stored.codec_profile_digest,
        artifact_profile_digest: stored.artifact_profile_digest,
        artifact_count: stored
            .artifact_count
            .map(|value| receipt_count_to_usize(value, &stored.receipt_id, "artifact_count"))
            .transpose()?,
        artifact_corruption_count: stored
            .artifact_corruption_count
            .map(|value| {
                receipt_count_to_usize(value, &stored.receipt_id, "artifact_corruption_count")
            })
            .transpose()?,
        artifact_missing_count: stored
            .artifact_missing_count
            .map(|value| {
                receipt_count_to_usize(value, &stored.receipt_id, "artifact_missing_count")
            })
            .transpose()?,
        vector_artifact_manifest_digest: stored.vector_artifact_manifest_digest,
        artifact_generation_id: stored.artifact_generation_id,
        approximate_scanned_count: stored
            .approximate_scanned_count
            .map(|value| {
                receipt_count_to_usize(value, &stored.receipt_id, "approximate_scanned_count")
            })
            .transpose()?,
        approximate_returned_count: stored
            .approximate_returned_count
            .map(|value| {
                receipt_count_to_usize(value, &stored.receipt_id, "approximate_returned_count")
            })
            .transpose()?,
        raw_rows_loaded_count: stored
            .raw_rows_loaded_count
            .map(|value| receipt_count_to_usize(value, &stored.receipt_id, "raw_rows_loaded_count"))
            .transpose()?,
        filter_strategy: stored.filter_strategy,
        vector_artifact_count: stored
            .vector_artifact_count
            .map(|value| receipt_count_to_usize(value, &stored.receipt_id, "vector_artifact_count"))
            .transpose()?,
        vector_artifact_missing_count: stored
            .vector_artifact_missing_count
            .map(|value| {
                receipt_count_to_usize(value, &stored.receipt_id, "vector_artifact_missing_count")
            })
            .transpose()?,
        vector_artifact_stale_count: stored
            .vector_artifact_stale_count
            .map(|value| {
                receipt_count_to_usize(value, &stored.receipt_id, "vector_artifact_stale_count")
            })
            .transpose()?,
        exact_rerank_count: stored
            .exact_rerank_count
            .map(|value| receipt_count_to_usize(value, &stored.receipt_id, "exact_rerank_count"))
            .transpose()?,
        approximate_candidate_count: stored
            .approximate_candidate_count
            .map(|value| {
                receipt_count_to_usize(value, &stored.receipt_id, "approximate_candidate_count")
            })
            .transpose()?,
        fallback_reason: stored.fallback_reason,
        approximate: stored.approximate,
        requested_candidates: receipt_count_to_usize(
            stored.requested_candidates,
            &stored.receipt_id,
            "requested_candidates",
        )?,
        returned_candidates: receipt_count_to_usize(
            stored.returned_candidates,
            &stored.receipt_id,
            "returned_candidates",
        )?,
        post_filter_candidates: receipt_count_to_usize(
            stored.post_filter_candidates,
            &stored.receipt_id,
            "post_filter_candidates",
        )?,
        fallback: stored.fallback,
        exact_rerank: stored.exact_rerank,
        result_ids: stored.result_ids,
        degradations: stored.degradations,
    })
}

/// Persist a search receipt as replay metadata.
///
/// SQLite rows remain authoritative for memory. This table stores only the
/// execution receipt and digest so the search can be addressed later.
pub fn store_search_receipt(
    conn: &Connection,
    receipt: &VectorSearchReceiptV1,
) -> Result<(), MemoryError> {
    let stored = stored_search_receipt(receipt)?;
    let receipt_json = serde_json::to_string(&stored)
        .map_err(|err| MemoryError::Other(format!("failed to serialize search receipt: {err}")))?;
    let receipt_digest = b3_digest(receipt_json.as_bytes());

    let existing_digest: Option<String> = conn
        .query_row(
            "SELECT receipt_digest FROM search_receipts WHERE receipt_id = ?1",
            params![&stored.receipt_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(existing_digest) = existing_digest {
        if existing_digest == receipt_digest {
            return Ok(());
        }
        return Err(MemoryError::SearchReceiptConflict {
            receipt_id: stored.receipt_id,
        });
    }

    let result_ids_json = serde_json::to_string(&stored.result_ids).map_err(|err| {
        MemoryError::Other(format!(
            "failed to serialize search receipt result IDs: {err}"
        ))
    })?;
    conn.execute(
        "INSERT INTO search_receipts (
            receipt_id,
            schema_version,
            evaluation_time,
            search_profile,
            candidate_backend,
            approximate,
            exact_rerank,
            fallback,
            requested_candidates,
            returned_candidates,
            post_filter_candidates,
            result_ids_json,
            receipt_json,
            receipt_digest
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            &stored.receipt_id,
            SEARCH_RECEIPT_SCHEMA_VERSION,
            stored.evaluation_time.to_rfc3339(),
            &stored.search_profile,
            &stored.candidate_backend,
            if stored.approximate { 1_i64 } else { 0_i64 },
            if stored.exact_rerank { 1_i64 } else { 0_i64 },
            &stored.fallback,
            receipt_count_to_i64(stored.requested_candidates, "requested_candidates")?,
            receipt_count_to_i64(stored.returned_candidates, "returned_candidates")?,
            receipt_count_to_i64(stored.post_filter_candidates, "post_filter_candidates")?,
            &result_ids_json,
            &receipt_json,
            &receipt_digest,
        ],
    )?;
    Ok(())
}

/// Load a durable search receipt by receipt/request ID.
pub fn get_search_receipt(
    conn: &Connection,
    receipt_id: &str,
) -> Result<Option<VectorSearchReceiptV1>, MemoryError> {
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT schema_version, receipt_json, receipt_digest
             FROM search_receipts
             WHERE receipt_id = ?1",
            params![receipt_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;

    let Some((schema_version, receipt_json, receipt_digest)) = row else {
        return Ok(None);
    };
    if schema_version != SEARCH_RECEIPT_SCHEMA_VERSION {
        return Err(MemoryError::CorruptData {
            table: "search_receipts",
            row_id: receipt_id.to_string(),
            detail: format!("unsupported receipt schema version '{schema_version}'"),
        });
    }

    let stored: StoredVectorSearchReceiptV1 =
        serde_json::from_str(&receipt_json).map_err(|err| MemoryError::CorruptData {
            table: "search_receipts",
            row_id: receipt_id.to_string(),
            detail: format!("invalid receipt JSON: {err}"),
        })?;
    let mut receipt = search_receipt_from_stored(stored)?;
    receipt.receipt_digest = Some(receipt_digest);
    Ok(Some(receipt))
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    column_sql: &str,
) -> Result<(), rusqlite::Error> {
    let pragma = format!("PRAGMA table_info({table})");
    let mut stmt = conn.prepare(&pragma)?;
    let exists = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .any(|name| name == column);

    if !exists {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {column_sql}"),
            [],
        )?;
    }

    Ok(())
}

/// Check and update the embedding metadata singleton row.
pub fn check_embedding_metadata(
    conn: &Connection,
    config: &EmbeddingConfig,
) -> Result<(), MemoryError> {
    // INTENTIONAL: row absent on first run before metadata is inserted
    let existing: Option<(String, usize)> = conn
        .query_row(
            "SELECT model_name, dimensions FROM embedding_metadata WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();

    match existing {
        Some((model, dims)) => {
            if model != config.model || dims != config.dimensions {
                tracing::warn!(
                    stored_model = %model,
                    stored_dims = dims,
                    configured_model = %config.model,
                    configured_dims = config.dimensions,
                    "Embedding model changed. Existing embeddings are stale."
                );
                conn.execute(
                    "UPDATE embedding_metadata
                     SET model_name = ?1,
                         dimensions = ?2,
                         embeddings_dirty = 1,
                         updated_at = datetime('now')
                     WHERE id = 1",
                    params![config.model, config.dimensions],
                )?;
            }
        }
        None => {
            conn.execute(
                "INSERT INTO embedding_metadata (id, model_name, dimensions) VALUES (1, ?1, ?2)",
                params![config.model, config.dimensions],
            )?;
        }
    }

    Ok(())
}

/// Encode an f32 slice as bytes for SQLite BLOB storage.
pub fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    encode_f32_le(embedding)
}

/// Encode f32 values as a stable little-endian persisted representation.
pub fn encode_f32_le(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Validate an embedding vector before it is stored or indexed.
pub(crate) fn validate_embedding(values: &[f32], expected_dim: usize) -> Result<(), MemoryError> {
    if values.len() != expected_dim {
        return Err(MemoryError::EmbeddingDimensionMismatch {
            expected: expected_dim,
            actual: values.len(),
        });
    }
    if let Some((index, _)) = values
        .iter()
        .enumerate()
        .find(|(_, value)| !value.is_finite())
    {
        return Err(MemoryError::NonFiniteEmbeddingValue { index });
    }
    Ok(())
}

/// Validate a returned embedding batch against the requested input count.
pub(crate) fn validate_embedding_batch(
    values: &[Vec<f32>],
    requested: usize,
    expected_dim: usize,
) -> Result<(), MemoryError> {
    if values.len() != requested {
        return Err(MemoryError::EmbeddingBatchCountMismatch {
            requested,
            returned: values.len(),
        });
    }
    for embedding in values {
        validate_embedding(embedding, expected_dim)?;
    }
    Ok(())
}

/// Validate the exact byte length of a persisted f32 vector blob.
pub(crate) fn validate_vector_blob_len(
    bytes: &[u8],
    expected_dim: usize,
) -> Result<(), MemoryError> {
    let expected_bytes = expected_dim
        .checked_mul(4)
        .ok_or_else(|| MemoryError::InvalidConfig {
            field: "embedding.dimensions",
            reason: "dimension byte length overflow".to_string(),
        })?;
    if bytes.len() != expected_bytes {
        return Err(MemoryError::VectorBlobLengthMismatch {
            expected_bytes,
            actual_bytes: bytes.len(),
        });
    }
    Ok(())
}

/// Decode a stable little-endian f32 persisted representation.
#[allow(clippy::manual_is_multiple_of)]
pub fn decode_f32_le(bytes: &[u8], expected_dim: usize) -> Result<Vec<f32>, MemoryError> {
    validate_vector_blob_len(bytes, expected_dim)?;
    decode_f32_le_unchecked_dim(bytes)
}

/// Decode a SQLite embedding BLOB back to f32 values.
#[allow(clippy::manual_is_multiple_of)]
pub fn bytes_to_embedding(bytes: &[u8]) -> Result<Vec<f32>, MemoryError> {
    if bytes.len() % 4 != 0 {
        return Err(MemoryError::InvalidEmbedding {
            expected_bytes: bytes.len() - (bytes.len() % 4),
            actual_bytes: bytes.len(),
        });
    }

    decode_f32_le_unchecked_dim(bytes)
}

fn decode_f32_le_unchecked_dim(bytes: &[u8]) -> Result<Vec<f32>, MemoryError> {
    let mut embedding = Vec::with_capacity(bytes.len() / 4);
    for (index, chunk) in bytes.chunks_exact(4).enumerate() {
        let value = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        if !value.is_finite() {
            return Err(MemoryError::NonFiniteEmbeddingValue { index });
        }
        embedding.push(value);
    }
    Ok(embedding)
}

pub fn is_embeddings_dirty(conn: &Connection) -> Result<bool, MemoryError> {
    let dirty: i32 = conn
        .query_row(
            "SELECT COALESCE(embeddings_dirty, 0) FROM embedding_metadata WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    Ok(dirty != 0)
}

pub fn clear_embeddings_dirty(conn: &Connection) -> Result<(), MemoryError> {
    conn.execute(
        "UPDATE embedding_metadata SET embeddings_dirty = 0 WHERE id = 1",
        [],
    )?;
    Ok(())
}

#[cfg(feature = "hnsw")]
pub(crate) fn queue_pending_index_op(
    tx: &rusqlite::Transaction<'_>,
    item_key: &str,
    entity_type: &str,
    op_kind: IndexOpKind,
) -> Result<(), MemoryError> {
    tx.execute(
        "INSERT INTO pending_index_ops (item_key, entity_type, op_kind, attempt_count, last_error, updated_at)
         VALUES (?1, ?2, ?3, 0, NULL, datetime('now'))
         ON CONFLICT(item_key) DO UPDATE SET
             entity_type = excluded.entity_type,
             op_kind = excluded.op_kind,
             attempt_count = 0,
             last_error = NULL,
             updated_at = datetime('now')",
        params![item_key, entity_type, op_kind.as_str()],
    )?;
    mark_sidecar_dirty(tx)?;
    Ok(())
}

#[cfg(feature = "hnsw")]
pub(crate) use IndexOpKind as PendingIndexOpKind;

#[cfg(feature = "hnsw")]
pub(crate) fn enqueue_pending_index_op(
    tx: &rusqlite::Transaction<'_>,
    item_key: &str,
    entity_type: &str,
    op_kind: PendingIndexOpKind,
) -> Result<(), MemoryError> {
    queue_pending_index_op(tx, item_key, entity_type, op_kind)
}

pub(crate) fn list_pending_index_ops(
    conn: &Connection,
) -> Result<Vec<PendingIndexOp>, MemoryError> {
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='pending_index_ops'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !table_exists {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        "SELECT item_key, entity_type, op_kind, attempt_count, last_error
         FROM pending_index_ops
         ORDER BY updated_at ASC, item_key ASC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let item_key: String = row.get(0)?;
            let op_kind: String = row.get(2)?;
            Ok(PendingIndexOp {
                item_key: item_key.clone(),
                entity_type: row.get(1)?,
                op_kind: IndexOpKind::parse(&op_kind, &item_key).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
                attempt_count: row.get::<_, i64>(3)? as u32,
                last_error: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(feature = "hnsw")]
pub(crate) fn pending_index_op_count(conn: &Connection) -> Result<usize, MemoryError> {
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='pending_index_ops'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !table_exists {
        return Ok(0);
    }

    let count: i64 = conn.query_row("SELECT COUNT(*) FROM pending_index_ops", [], |row| {
        row.get(0)
    })?;
    Ok(count as usize)
}

#[cfg(feature = "hnsw")]
pub(crate) fn mark_pending_index_ops_failed(
    conn: &Connection,
    item_keys: &[String],
    error: &str,
) -> Result<(), MemoryError> {
    with_transaction(conn, |tx| {
        for item_key in item_keys {
            tx.execute(
                "UPDATE pending_index_ops
                 SET attempt_count = attempt_count + 1,
                     last_error = ?1,
                     updated_at = datetime('now')
                 WHERE item_key = ?2",
                params![error, item_key],
            )?;
        }
        Ok(())
    })
}

#[cfg(feature = "hnsw")]
pub(crate) fn clear_pending_index_ops(
    conn: &Connection,
    item_keys: &[String],
) -> Result<(), MemoryError> {
    with_transaction(conn, |tx| {
        for item_key in item_keys {
            tx.execute(
                "DELETE FROM pending_index_ops WHERE item_key = ?1",
                params![item_key],
            )?;
        }
        Ok(())
    })
}

#[cfg(feature = "hnsw")]
pub(crate) fn clear_all_pending_index_ops(conn: &Connection) -> Result<(), MemoryError> {
    conn.execute("DELETE FROM pending_index_ops", [])?;
    Ok(())
}

#[cfg(feature = "hnsw")]
pub(crate) fn load_embedding_for_index_key(
    conn: &Connection,
    item_key: &str,
) -> Result<Option<Vec<f32>>, MemoryError> {
    let Some((domain, raw_id)) = item_key.split_once(':') else {
        return Err(MemoryError::InvalidKey(item_key.to_string()));
    };

    let blob_result: Result<Option<Vec<u8>>, rusqlite::Error> = match domain {
        "fact" => conn.query_row(
            "SELECT embedding FROM facts WHERE id = ?1",
            params![raw_id],
            |row| row.get(0),
        ),
        "chunk" => conn.query_row(
            "SELECT embedding FROM chunks WHERE id = ?1",
            params![raw_id],
            |row| row.get(0),
        ),
        "msg" => {
            let message_id = raw_id
                .parse::<i64>()
                .map_err(|e| MemoryError::InvalidKey(format!("{}: {e}", item_key)))?;
            conn.query_row(
                "SELECT embedding FROM messages WHERE id = ?1",
                params![message_id],
                |row| row.get(0),
            )
        }
        "episode" => conn.query_row(
            "SELECT embedding FROM episodes WHERE episode_id = ?1",
            params![raw_id],
            |row| row.get(0),
        ),
        _ => return Err(MemoryError::InvalidKey(item_key.to_string())),
    };

    let blob = match blob_result {
        Ok(blob) => blob,
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(err) => return Err(err.into()),
    };

    blob.map(|bytes| bytes_to_embedding(&bytes)).transpose()
}

#[cfg(feature = "hnsw")]
fn mark_sidecar_dirty(tx: &rusqlite::Transaction<'_>) -> Result<(), MemoryError> {
    tx.execute(
        "INSERT INTO hnsw_metadata (key, value) VALUES ('sidecar_dirty', '1')
         ON CONFLICT(key) DO UPDATE SET value = '1'",
        [],
    )?;
    Ok(())
}

#[cfg(feature = "hnsw")]
pub(crate) fn is_sidecar_dirty(conn: &Connection) -> Result<bool, MemoryError> {
    // INTENTIONAL: row absent when HNSW metadata has not been written yet
    let dirty: Option<String> = conn
        .query_row(
            "SELECT value FROM hnsw_metadata WHERE key = 'sidecar_dirty'",
            [],
            |row| row.get(0),
        )
        .ok();
    Ok(matches!(dirty.as_deref(), Some("1")))
}

#[cfg(feature = "hnsw")]
pub(crate) fn set_sidecar_dirty(conn: &Connection, dirty: bool) -> Result<(), MemoryError> {
    conn.execute(
        "INSERT INTO hnsw_metadata (key, value) VALUES ('sidecar_dirty', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![if dirty { "1" } else { "0" }],
    )?;
    Ok(())
}

pub(crate) fn parse_optional_json(
    table: &'static str,
    row_id: &str,
    field: &'static str,
    raw: Option<&str>,
) -> Result<Option<serde_json::Value>, MemoryError> {
    match raw {
        Some(raw) => serde_json::from_str(raw)
            .map(Some)
            .map_err(|e| MemoryError::CorruptData {
                table,
                row_id: row_id.to_string(),
                detail: format!("invalid {field}: {e}"),
            }),
        None => Ok(None),
    }
}

pub(crate) fn parse_string_list_json(
    table: &'static str,
    row_id: &str,
    field: &'static str,
    raw: &str,
) -> Result<Vec<String>, MemoryError> {
    serde_json::from_str(raw).map_err(|e| MemoryError::CorruptData {
        table,
        row_id: row_id.to_string(),
        detail: format!("invalid {field}: {e}"),
    })
}

pub(crate) fn parse_role(
    table: &'static str,
    row_id: &str,
    raw: &str,
) -> Result<Role, MemoryError> {
    Role::from_str_value(raw).ok_or_else(|| MemoryError::CorruptData {
        table,
        row_id: row_id.to_string(),
        detail: format!("invalid role '{raw}'"),
    })
}

pub(crate) fn parse_episode_outcome(
    row_id: &str,
    raw: &str,
) -> Result<EpisodeOutcome, MemoryError> {
    EpisodeOutcome::from_str_value(raw).ok_or_else(|| MemoryError::CorruptData {
        table: "episodes",
        row_id: row_id.to_string(),
        detail: format!("invalid outcome '{raw}'"),
    })
}

pub(crate) fn parse_verification_status(
    row_id: &str,
    raw: &str,
) -> Result<VerificationStatus, MemoryError> {
    serde_json::from_str(raw).map_err(|e| MemoryError::CorruptData {
        table: "episodes",
        row_id: row_id.to_string(),
        detail: format!("invalid verification_status: {e}"),
    })
}

/// Run integrity verification on the database.
pub fn verify_integrity_sync(
    conn: &Connection,
    mode: VerifyMode,
) -> Result<IntegrityReport, MemoryError> {
    let mut issues = Vec::new();

    let schema_version: u32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap_or_else(|e| {
            issues.push(format!("failed to read schema version: {e}"));
            0
        });
    if schema_version > MAX_SCHEMA_VERSION {
        issues.push(format!(
            "schema version {} is ahead of supported {}",
            schema_version, MAX_SCHEMA_VERSION
        ));
    }

    let fact_count: usize = conn
        .query_row("SELECT COUNT(*) FROM facts", [], |row| row.get(0))
        .unwrap_or_else(|e| {
            issues.push(format!("failed to count facts: {e}"));
            0
        });
    let chunk_count: usize = conn
        .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
        .unwrap_or_else(|e| {
            issues.push(format!("failed to count chunks: {e}"));
            0
        });
    let message_count: usize = conn
        .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
        .unwrap_or_else(|e| {
            issues.push(format!("failed to count messages: {e}"));
            0
        });
    let episode_count: usize = conn
        .query_row("SELECT COUNT(*) FROM episodes", [], |row| row.get(0))
        .unwrap_or_else(|e| {
            issues.push(format!("failed to count episodes: {e}"));
            0
        });

    let facts_missing_embeddings: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM facts WHERE embedding IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|e| {
            issues.push(format!("failed to count facts missing embeddings: {e}"));
            0
        });
    let chunks_missing_embeddings: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM chunks WHERE embedding IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|e| {
            issues.push(format!("failed to count chunks missing embeddings: {e}"));
            0
        });
    let episodes_missing_embeddings: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM episodes WHERE embedding IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|e| {
            issues.push(format!("failed to count episodes missing embeddings: {e}"));
            0
        });

    if facts_missing_embeddings > 0 {
        issues.push(format!(
            "{} facts missing embeddings",
            facts_missing_embeddings
        ));
    }
    if chunks_missing_embeddings > 0 {
        issues.push(format!(
            "{} chunks missing embeddings",
            chunks_missing_embeddings
        ));
    }
    if episodes_missing_embeddings > 0 {
        issues.push(format!(
            "{} episodes missing embeddings",
            episodes_missing_embeddings
        ));
    }

    let pending_ops = list_pending_index_ops(conn).unwrap_or_default();
    if !pending_ops.is_empty() {
        issues.push(format!(
            "{} pending HNSW sidecar ops queued in SQLite",
            pending_ops.len()
        ));
        for op in pending_ops.iter().take(5) {
            let op_kind = op.op_kind.as_str();
            let detail = match &op.last_error {
                Some(last_error) => format!(
                    "{} {} {} (attempts: {}, last_error: {})",
                    op.entity_type,
                    op.op_kind.as_str(),
                    op.item_key,
                    op.attempt_count,
                    last_error
                ),
                None => format!(
                    "{} {} {} (attempts: {})",
                    op.entity_type, op_kind, op.item_key, op.attempt_count
                ),
            };
            issues.push(format!("pending sidecar op: {detail}"));
        }
    }

    if mode == VerifyMode::Full {
        let dims: usize = conn
            .query_row(
                "SELECT dimensions FROM embedding_metadata WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|e| {
                issues.push(format!("failed to read embedding dimensions: {e}"));
                0
            });

        verify_fts_drift(conn, "facts", "facts_rowid_map", fact_count, &mut issues);
        verify_fts_drift(conn, "chunks", "chunks_rowid_map", chunk_count, &mut issues);
        verify_fts_drift(
            conn,
            "messages",
            "messages_rowid_map",
            message_count,
            &mut issues,
        );
        verify_fts_drift(
            conn,
            "episodes",
            "episodes_rowid_map",
            episode_count,
            &mut issues,
        );

        verify_blob_table(conn, "facts", "id", "embedding", dims, &mut issues)?;
        verify_blob_table(conn, "chunks", "id", "embedding", dims, &mut issues)?;
        verify_blob_table(conn, "messages", "id", "embedding", dims, &mut issues)?;
        verify_blob_table(
            conn,
            "episodes",
            "episode_id",
            "embedding",
            dims,
            &mut issues,
        )?;

        verify_quantized_table(conn, "facts", "id", dims, &mut issues)?;
        verify_quantized_table(conn, "chunks", "id", dims, &mut issues)?;
        verify_quantized_table(conn, "messages", "id", dims, &mut issues)?;
        verify_quantized_table(conn, "episodes", "episode_id", dims, &mut issues)?;

        verify_session_rows(conn, &mut issues)?;
        verify_message_rows(conn, &mut issues)?;
        verify_fact_rows(conn, &mut issues)?;
        verify_document_rows(conn, &mut issues)?;
        verify_episode_rows(conn, &mut issues)?;

        let integrity_check: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap_or_else(|_| "error".to_string());
        if integrity_check != "ok" {
            issues.push(format!("SQLite integrity_check: {}", integrity_check));
        }
    }

    Ok(IntegrityReport {
        ok: issues.is_empty(),
        schema_version,
        fact_count,
        chunk_count,
        message_count,
        facts_missing_embeddings,
        chunks_missing_embeddings,
        issues,
    })
}

/// Reconcile FTS indexes by rebuilding them from source data.
pub fn reconcile_fts(conn: &Connection) -> Result<(), MemoryError> {
    with_transaction(conn, |tx| {
        tx.execute_batch("DROP TABLE IF EXISTS facts_fts")?;
        tx.execute_batch("DELETE FROM facts_rowid_map")?;
        tx.execute_batch(
            "CREATE VIRTUAL TABLE facts_fts USING fts5(
                content,
                content='',
                content_rowid='rowid',
                tokenize='porter unicode61'
            )",
        )?;
        tx.execute_batch("INSERT INTO facts_rowid_map (fact_id) SELECT id FROM facts")?;
        tx.execute_batch(
            "INSERT INTO facts_fts (rowid, content)
             SELECT rm.rowid, f.content
             FROM facts_rowid_map rm
             JOIN facts f ON f.id = rm.fact_id",
        )?;

        tx.execute_batch("DROP TABLE IF EXISTS chunks_fts")?;
        tx.execute_batch("DELETE FROM chunks_rowid_map")?;
        tx.execute_batch(
            "CREATE VIRTUAL TABLE chunks_fts USING fts5(
                content,
                content='',
                content_rowid='rowid',
                tokenize='porter unicode61'
            )",
        )?;
        tx.execute_batch("INSERT INTO chunks_rowid_map (chunk_id) SELECT id FROM chunks")?;
        tx.execute_batch(
            "INSERT INTO chunks_fts (rowid, content)
             SELECT rm.rowid, c.content
             FROM chunks_rowid_map rm
             JOIN chunks c ON c.id = rm.chunk_id",
        )?;

        tx.execute_batch("DROP TABLE IF EXISTS messages_fts")?;
        tx.execute_batch("DELETE FROM messages_rowid_map")?;
        tx.execute_batch(
            "CREATE VIRTUAL TABLE messages_fts USING fts5(
                content,
                content='',
                content_rowid='rowid',
                tokenize='porter unicode61'
            )",
        )?;
        tx.execute_batch("INSERT INTO messages_rowid_map (message_id) SELECT id FROM messages")?;
        tx.execute_batch(
            "INSERT INTO messages_fts (rowid, content)
             SELECT rm.rowid, m.content
             FROM messages_rowid_map rm
             JOIN messages m ON m.id = rm.message_id",
        )?;

        tx.execute_batch("DROP TABLE IF EXISTS episodes_fts")?;
        tx.execute_batch("DELETE FROM episodes_rowid_map")?;
        tx.execute_batch(
            "CREATE VIRTUAL TABLE episodes_fts USING fts5(
                content,
                content='',
                content_rowid='rowid',
                tokenize='porter unicode61'
            )",
        )?;
        tx.execute_batch(
            "INSERT INTO episodes_rowid_map (episode_id, document_id) SELECT episode_id, document_id FROM episodes",
        )?;
        tx.execute_batch(
            "INSERT INTO episodes_fts (rowid, content)
             SELECT rm.rowid, e.search_text
             FROM episodes_rowid_map rm
             JOIN episodes e ON e.episode_id = rm.episode_id",
        )?;

        Ok(())
    })?;

    tracing::info!("FTS indexes reconciled");
    Ok(())
}

fn verify_fts_drift(
    conn: &Connection,
    label: &str,
    map_table: &str,
    source_count: usize,
    issues: &mut Vec<String>,
) {
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name = ?1",
            params![map_table],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !table_exists {
        if source_count > 0 {
            issues.push(format!("{} rows exist but {} is missing", label, map_table));
        }
        return;
    }

    let sql = format!("SELECT COUNT(*) FROM {}", map_table);
    let indexed_count: usize = conn.query_row(&sql, [], |row| row.get(0)).unwrap_or(0);
    if indexed_count != source_count {
        issues.push(format!(
            "FTS {} index drift: {} rows in map vs {} source rows",
            label, indexed_count, source_count
        ));
    }
}

fn verify_blob_table(
    conn: &Connection,
    table: &'static str,
    id_column: &'static str,
    blob_column: &'static str,
    expected_dims: usize,
    issues: &mut Vec<String>,
) -> Result<(), MemoryError> {
    if expected_dims == 0 {
        return Ok(());
    }

    let sql = format!(
        "SELECT CAST({id_column} AS TEXT), {blob_column} FROM {table} WHERE {blob_column} IS NOT NULL"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
    })?;

    for row in rows {
        let (row_id, blob) = row?;
        match bytes_to_embedding(&blob) {
            Ok(embedding) if embedding.len() != expected_dims => issues.push(format!(
                "{}({}) has embedding dimension {} but expected {}",
                table,
                row_id,
                embedding.len(),
                expected_dims
            )),
            Ok(_) => {}
            Err(err) => issues.push(format!(
                "{}({}) invalid embedding blob: {}",
                table, row_id, err
            )),
        }
    }

    Ok(())
}

fn verify_quantized_table(
    conn: &Connection,
    table: &'static str,
    id_column: &'static str,
    expected_dims: usize,
    issues: &mut Vec<String>,
) -> Result<(), MemoryError> {
    if expected_dims == 0 {
        return Ok(());
    }

    let sql = format!(
        "SELECT CAST({id_column} AS TEXT), embedding_q8 FROM {table} WHERE embedding IS NOT NULL"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<Vec<u8>>>(1)?))
    })?;

    for row in rows {
        let (row_id, blob) = row?;
        match blob {
            Some(blob) => {
                if let Err(err) = unpack_quantized(&blob, expected_dims) {
                    issues.push(format!(
                        "{}({}) invalid quantized embedding: {}",
                        table, row_id, err
                    ));
                }
            }
            None => issues.push(format!("{}({}) missing quantized embedding", table, row_id)),
        }
    }

    Ok(())
}

fn verify_session_rows(conn: &Connection, issues: &mut Vec<String>) -> Result<(), MemoryError> {
    let mut stmt = conn.prepare("SELECT id, metadata FROM sessions WHERE metadata IS NOT NULL")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, metadata) = row?;
        if let Err(err) = parse_optional_json("sessions", &id, "metadata", Some(&metadata)) {
            issues.push(err.to_string());
        }
    }
    Ok(())
}

fn verify_message_rows(conn: &Connection, issues: &mut Vec<String>) -> Result<(), MemoryError> {
    let mut stmt = conn.prepare("SELECT id, role, metadata FROM messages")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    for row in rows {
        let (id, role, metadata) = row?;
        let row_id = id.to_string();
        if let Err(err) = parse_role("messages", &row_id, &role) {
            issues.push(err.to_string());
        }
        if let Err(err) = parse_optional_json("messages", &row_id, "metadata", metadata.as_deref())
        {
            issues.push(err.to_string());
        }
    }
    Ok(())
}

fn verify_fact_rows(conn: &Connection, issues: &mut Vec<String>) -> Result<(), MemoryError> {
    let mut stmt = conn.prepare("SELECT id, metadata FROM facts WHERE metadata IS NOT NULL")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, metadata) = row?;
        if let Err(err) = parse_optional_json("facts", &id, "metadata", Some(&metadata)) {
            issues.push(err.to_string());
        }
    }
    Ok(())
}

fn verify_document_rows(conn: &Connection, issues: &mut Vec<String>) -> Result<(), MemoryError> {
    let mut stmt = conn.prepare("SELECT id, metadata FROM documents WHERE metadata IS NOT NULL")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, metadata) = row?;
        if let Err(err) = parse_optional_json("documents", &id, "metadata", Some(&metadata)) {
            issues.push(err.to_string());
        }
    }
    Ok(())
}

fn verify_episode_rows(conn: &Connection, issues: &mut Vec<String>) -> Result<(), MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT episode_id, cause_ids, outcome, verification_status
         FROM episodes",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (episode_id, cause_ids, outcome, verification_status) = row?;
        if let Err(err) = parse_string_list_json("episodes", &episode_id, "cause_ids", &cause_ids) {
            issues.push(err.to_string());
        }
        if let Err(err) = parse_episode_outcome(&episode_id, &outcome) {
            issues.push(err.to_string());
        }
        if let Err(err) = parse_verification_status(&episode_id, &verification_status) {
            issues.push(err.to_string());
        }
    }
    Ok(())
}
