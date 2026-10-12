use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};

const APP_SCHEMA_VERSION: i32 = 4;
pub(crate) const NOTEBOOK_SCHEMA_VERSION: i32 = 7;

#[derive(Clone, Copy)]
pub(crate) enum DatabaseSchema {
    App,
    Notebook,
}

fn admission_io_error(error: std::io::Error) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR),
        Some(format!("Database schema inspection failed: {error}")),
    )
}

fn admission_sidecar(path: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    name.into()
}

#[derive(PartialEq, Eq)]
struct AdmissionFile {
    size: u64,
    digest: [u8; 32],
    modified: std::time::SystemTime,
    #[cfg(unix)]
    identity: (u64, u64),
}

#[cfg(test)]
thread_local! {
    static AFTER_INSPECTION_COPY: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = std::cell::RefCell::new(None);
    static BEFORE_SOURCE_ACQUISITION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = std::cell::RefCell::new(None);
}

fn admission_files(path: &std::path::Path) -> rusqlite::Result<Vec<Option<AdmissionFile>>> {
    use std::io::Read;
    ["", "-wal", "-shm", "-journal"]
        .into_iter()
        .map(|suffix| {
            let mut file = match std::fs::File::open(admission_sidecar(path, suffix)) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(admission_io_error(error)),
            };
            let metadata = file.metadata().map_err(admission_io_error)?;
            let mut hash = Sha256::new();
            let mut buffer = [0; 65536];
            loop {
                let length = file.read(&mut buffer).map_err(admission_io_error)?;
                if length == 0 {
                    break;
                }
                hash.update(&buffer[..length]);
            }
            Ok(Some(AdmissionFile {
                size: metadata.len(),
                digest: hash.finalize().into(),
                modified: metadata.modified().map_err(admission_io_error)?,
                #[cfg(unix)]
                identity: {
                    use std::os::unix::fs::MetadataExt;
                    (metadata.dev(), metadata.ino())
                },
            }))
        })
        .collect()
}

/// Inspect committed schema on a disposable SQLite copy before a source handle
/// is allowed to initialize SHM or checkpoint on close. The copy is never an
/// admitted store: SQLite alone interprets its WAL/journal and it is discarded.
/// Changes during inspection fail closed; callers must also validate the actual
/// source connection because inspection does not fence an external writer.
pub(crate) fn inspect_schema_for_admission(
    path: &std::path::Path,
    schema: DatabaseSchema,
) -> rusqlite::Result<()> {
    let source = match std::fs::canonicalize(path) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if admission_files(path)?.iter().skip(1).any(Option::is_some) {
                return Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
                    Some("Database main file is missing while recovery sidecars exist".into()),
                ));
            }
            return Ok(()); // New database creation belongs to the normal owner.
        }
        Err(error) => return Err(admission_io_error(error)),
    };
    let before = admission_files(&source)?;
    if before[0].as_ref().is_some_and(|file| file.size == 0)
        && before[1].as_ref().is_some_and(|file| file.size != 0)
    {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
            Some("Empty database main file has retained WAL data; inspection refused".into()),
        ));
    }
    let inspection = tempfile::tempdir().map_err(admission_io_error)?;
    let copied = inspection.path().join("schema-inspection.db");
    for (index, suffix) in [(0, ""), (1, "-wal"), (3, "-journal")] {
        if before[index].is_some() {
            std::fs::copy(
                admission_sidecar(&source, suffix),
                admission_sidecar(&copied, suffix),
            )
            .map_err(admission_io_error)?;
        }
    }
    #[cfg(test)]
    AFTER_INSPECTION_COPY.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    let result = {
        let connection = Connection::open(&copied)?;
        validate_schema_version(&connection, schema).map(|_| ())
    };
    if admission_files(&source)? != before {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            Some("Database changed during schema inspection; admission refused".into()),
        ));
    }
    result
}

/// Canonical app/notebook writable acquisition. Real-handle validation prevents
/// migration of a newer schema observed after inspection. Disabling close-time
/// checkpoints protects rejected handles; runtime pragmas restore normal close
/// behavior once their own schema guard succeeds. This does not claim an atomic
/// fence against arbitrary external schema writers or SHM changes on that race.
pub(crate) fn open_connection(
    path: &std::path::Path,
    schema: DatabaseSchema,
) -> rusqlite::Result<Connection> {
    inspect_schema_for_admission(path, schema)?;
    #[cfg(test)]
    BEFORE_SOURCE_ACQUISITION.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    let connection = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.set_db_config(
        rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
        true,
    )?;
    validate_schema_version(&connection, schema)?;
    Ok(connection)
}

/// Apply pragmas for performance and correctness.
///
/// Four issues fixed from hostile audit:
/// 1. `synchronous=NORMAL` was missing — required for WAL mode to avoid
///    unnecessary fsync on every commit while still protecting against
///    database corruption (WAL + NORMAL is safe; FULL is only needed
///    for rollback-journal mode).
/// 2. `cache_size` was missing — set a reasonable negative value (KiB)
///    to allow the page cache to grow beyond the tiny default.
/// 3. `mmap_size` was missing — enable memory-mapped I/O for read-heavy
///    workloads (FTS5, HNSW lookups).
/// 4. `temp_store=MEMORY` was missing — keep temp tables/indexes in RAM.
pub fn apply_pragmas(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA foreign_keys=ON;
         PRAGMA busy_timeout=15000;
         PRAGMA synchronous=NORMAL;
         PRAGMA cache_size=-65536;
         PRAGMA mmap_size=67108864;
         PRAGMA temp_store=MEMORY;
         PRAGMA wal_autocheckpoint=1000;",
    )?;
    conn.set_db_config(
        rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
        false,
    )?;
    Ok(())
}

/// Create or migrate the app-level database schema.
pub fn migrate_app_db(conn: &Connection) -> rusqlite::Result<()> {
    let version = validate_schema_version(conn, DatabaseSchema::App)?;
    apply_pragmas(conn)?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )?;

    if version < 1 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS notebooks (
                id            TEXT PRIMARY KEY,
                name          TEXT NOT NULL,
                description   TEXT,
                directory     TEXT NOT NULL,
                source_count  INTEGER DEFAULT 0,
                last_accessed TEXT,
                created_at    TEXT DEFAULT (datetime('now')),
                updated_at    TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS providers (
                id             TEXT PRIMARY KEY,
                enabled        INTEGER DEFAULT 0,
                base_url       TEXT,
                secret_ref     TEXT,
                last_refreshed TEXT,
                created_at     TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS models (
                id              TEXT NOT NULL,
                provider_id     TEXT NOT NULL REFERENCES providers(id),
                display_name    TEXT NOT NULL,
                parameter_size  TEXT,
                context_window  INTEGER,
                capabilities    TEXT,
                available       INTEGER DEFAULT 1,
                stale           INTEGER DEFAULT 0,
                last_error      TEXT,
                PRIMARY KEY (id, provider_id)
            );

            CREATE TABLE IF NOT EXISTS settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )?;

        // Insert default settings
        conn.execute_batch(
            "INSERT OR IGNORE INTO settings (key, value) VALUES ('default_provider', 'ollama');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('default_model', 'qwen3.5:4b');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('default_embedding_model', 'NomicEmbedTextV15');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('summary_mode', 'manual');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('memory_backend', 'gloss-local');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('memory_backend_fallback', 'true');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('semantic_memory_auto_project', 'false');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('semantic_memory_strict_testing', 'false');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('semantic_memory_turbo_quant_require_fresh_artifacts', 'true');
             -- v1/v2 default; v3 migration above flips this on upgrade.
             INSERT OR IGNORE INTO settings (key, value) VALUES ('semantic_memory_embedding_url', 'http://localhost:11434');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('semantic_memory_embedding_model', 'bge-m3');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('semantic_memory_embedding_timeout_secs', '10');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('semantic_memory_search_timeout_ms', '60000');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('generation_temperature', '0.7');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('generation_top_p', '');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('generation_top_k', '');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('generation_min_p', '');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('generation_repeat_penalty', '');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('ollama_url', 'http://localhost:11434');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('theme', 'system');",
        )?;

        // Insert default Ollama provider
        conn.execute_batch(
            "INSERT OR IGNORE INTO providers (id, enabled, base_url)
             VALUES ('ollama', 1, 'http://localhost:11434');",
        )?;

        set_schema_version(conn, 1)?;
    }

    if version < 2 {
        if !table_has_column(conn, "models", "available")? {
            conn.execute(
                "ALTER TABLE models ADD COLUMN available INTEGER DEFAULT 1",
                [],
            )?;
        }
        if !table_has_column(conn, "models", "stale")? {
            conn.execute("ALTER TABLE models ADD COLUMN stale INTEGER DEFAULT 0", [])?;
        }
        if !table_has_column(conn, "models", "last_error")? {
            conn.execute("ALTER TABLE models ADD COLUMN last_error TEXT", [])?;
        }
        set_schema_version(conn, 2)?;
    }

    // v3: Stop shipping FastEmbed as the default. In-process ONNX has caused
    // crashes during batch imports (per hostile audit / user memory: "ONNX
    // heap corruption kills Gloss during batch imports" + "user forbids
    // band-aids — only out-of-process embedding acceptable"). We flip any
    // existing user that still has 'fastembed' selected to 'ollama' on first
    // run of this migration, and change the new-install default to 'ollama'
    // so fresh installs never touch the dangerous path.
    if version < 3 {
        let flipped = conn.execute(
            "UPDATE settings
                SET value = 'ollama'
              WHERE key = 'semantic_memory_embedding_provider'
                AND lower(value) = 'fastembed'",
            [],
        )?;
        if flipped > 0 {
            eprintln!(
                "[gloss] migration v3: flipped {} setting(s) from \
                 'fastembed' to 'ollama' for crash-isolated embeddings. \
                 Use Settings → Embedding to change.",
                flipped
            );
        }
        // The new-install default below replaces the v1/v2 INSERT for the
        // embedding provider key. Existing users with a different value are
        // untouched.
        conn.execute(
            "INSERT OR IGNORE INTO settings (key, value) \
             VALUES ('semantic_memory_embedding_provider', 'ollama')",
            [],
        )?;
        set_schema_version(conn, 3)?;
    }

    // v4: the legacy 8-second semantic search default could expire before the
    // configured 10-second Ollama embedding request, and it was far below the
    // observed cold-start budget for proof-backed TurboQuant retrieval. Only
    // migrate the exact legacy default; explicit operator values are retained.
    if version < 4 {
        conn.execute(
            "UPDATE settings
                SET value = '60000'
              WHERE key = 'semantic_memory_search_timeout_ms'
                AND value = '8000'",
            [],
        )?;
        set_schema_version(conn, APP_SCHEMA_VERSION)?;
    }

    conn.execute_batch(
        "INSERT OR IGNORE INTO settings (key, value) VALUES ('semantic_memory_strict_testing', 'false');
         INSERT OR IGNORE INTO settings (key, value) VALUES ('semantic_memory_turbo_quant_require_fresh_artifacts', 'true');
         INSERT OR IGNORE INTO settings (key, value) VALUES ('generation_temperature', '0.7');
         INSERT OR IGNORE INTO settings (key, value) VALUES ('generation_top_p', '');
         INSERT OR IGNORE INTO settings (key, value) VALUES ('generation_top_k', '');
         INSERT OR IGNORE INTO settings (key, value) VALUES ('generation_min_p', '');
         INSERT OR IGNORE INTO settings (key, value) VALUES ('generation_repeat_penalty', '');
         INSERT OR IGNORE INTO settings (key, value) VALUES ('chunk_target_tokens', '1100');",
    )?;

    ensure_provider_rows(conn)?;

    Ok(())
}

fn ensure_provider_rows(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "INSERT OR IGNORE INTO providers (id, enabled, base_url)
         VALUES ('ollama', 1, 'http://localhost:11434');
         INSERT OR IGNORE INTO providers (id, enabled, base_url)
         VALUES ('openai', 0, 'https://api.openai.com/v1');
         INSERT OR IGNORE INTO providers (id, enabled, base_url)
         VALUES ('anthropic', 0, 'https://api.anthropic.com/v1');
         INSERT OR IGNORE INTO providers (id, enabled, base_url)
         VALUES ('llamacpp', 0, 'http://localhost:8080/v1');",
    )
}

/// Create or migrate a per-notebook database schema.
pub fn migrate_notebook_db(conn: &Connection) -> rusqlite::Result<()> {
    let version = validate_schema_version(conn, DatabaseSchema::Notebook)?;
    apply_pragmas(conn)?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )?;

    if version < 1 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sources (
                id                TEXT PRIMARY KEY,
                source_type       TEXT NOT NULL,
                title             TEXT NOT NULL,
                original_filename TEXT,
                file_hash         TEXT,
                url               TEXT,
                file_path         TEXT,
                content_text      TEXT,
                word_count        INTEGER,
                metadata          TEXT,
                summary           TEXT,
                summary_model     TEXT,
                status            TEXT DEFAULT 'pending',
                error_message     TEXT,
                selected          INTEGER DEFAULT 1,
                created_at        TEXT DEFAULT (datetime('now')),
                updated_at        TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS chunks (
                id              TEXT PRIMARY KEY,
                source_id       TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
                chunk_index     INTEGER NOT NULL,
                content         TEXT NOT NULL,
                token_count     INTEGER,
                start_offset    INTEGER,
                end_offset      INTEGER,
                metadata        TEXT,
                embedding_id    INTEGER,
                embedding_model TEXT,
                created_at      TEXT DEFAULT (datetime('now'))
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
                content,
                content='chunks',
                content_rowid='rowid'
            );

            CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
                INSERT INTO chunks_fts(rowid, content) VALUES (new.rowid, new.content);
            END;

            CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
            END;

            CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE OF content ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
                INSERT INTO chunks_fts(rowid, content) VALUES (new.rowid, new.content);
            END;

            CREATE TABLE IF NOT EXISTS conversations (
                id          TEXT PRIMARY KEY,
                title       TEXT,
                style       TEXT DEFAULT 'default',
                custom_goal TEXT,
                created_at  TEXT DEFAULT (datetime('now')),
                updated_at  TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS messages (
                id                  TEXT PRIMARY KEY,
                conversation_id     TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                role                TEXT NOT NULL,
                content             TEXT NOT NULL,
                citations           TEXT,
                model_used          TEXT,
                tokens_prompt       INTEGER,
                tokens_response     INTEGER,
                created_at          TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS chat_attempts (
                attempt_id TEXT PRIMARY KEY,
                notebook_id TEXT NOT NULL,
                conversation_id TEXT NOT NULL,
                assistant_message_id TEXT NOT NULL,
                user_message_id TEXT,
                provider TEXT,
                model TEXT,
                status TEXT NOT NULL,
                phase TEXT,
                error_code TEXT,
                error_message TEXT,
                request_digest TEXT,
                response_digest TEXT,
                partial_policy TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                terminal_at TEXT
            );

            CREATE TABLE IF NOT EXISTS notes (
                id          TEXT PRIMARY KEY,
                title       TEXT,
                content     TEXT NOT NULL,
                note_type   TEXT NOT NULL,
                citations   TEXT,
                pinned      INTEGER DEFAULT 0,
                source_id   TEXT,
                created_at  TEXT DEFAULT (datetime('now')),
                updated_at  TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS studio_outputs (
                id            TEXT PRIMARY KEY,
                output_type   TEXT NOT NULL,
                title         TEXT,
                prompt_used   TEXT NOT NULL,
                raw_content   TEXT,
                config        TEXT,
                source_ids    TEXT,
                file_path     TEXT,
                status        TEXT DEFAULT 'pending',
                error_message TEXT,
                created_at    TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS notebook_config (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            -- Indexes
            CREATE INDEX IF NOT EXISTS idx_chunks_source ON chunks(source_id);
            CREATE INDEX IF NOT EXISTS idx_chunks_embedding_id ON chunks(embedding_id);
            CREATE INDEX IF NOT EXISTS idx_messages_conversation ON messages(conversation_id, created_at);
            CREATE INDEX IF NOT EXISTS idx_sources_status ON sources(status);
            CREATE INDEX IF NOT EXISTS idx_sources_hash ON sources(file_hash);
            CREATE INDEX IF NOT EXISTS idx_notes_pinned ON notes(pinned DESC, updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_studio_type ON studio_outputs(output_type, created_at DESC);",
        )?;

        // Default notebook config
        conn.execute_batch(
            "INSERT OR IGNORE INTO notebook_config (key, value) VALUES ('default_style', 'default');
             INSERT OR IGNORE INTO notebook_config (key, value) VALUES ('custom_goal', '');
             INSERT OR IGNORE INTO notebook_config (key, value) VALUES ('response_length', 'default');
             INSERT OR IGNORE INTO notebook_config (key, value) VALUES ('output_language', 'en');",
        )?;

        set_schema_version(conn, 1)?;
    }

    if version < 2 {
        // Add embedding_id index for HNSW lookups (hot path for chunk retrieval)
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_chunks_embedding_id ON chunks(embedding_id);",
        )?;
        set_schema_version(conn, 2)?;
    }

    if version < 3 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS semantic_memory_links (
                chunk_id TEXT PRIMARY KEY,
                notebook_id TEXT NOT NULL,
                source_id TEXT NOT NULL,
                sm_document_id TEXT,
                sm_chunk_id TEXT,
                sm_episode_id TEXT,
                gloss_chunk_id TEXT,
                projection_unit_id TEXT,
                projection_unit_kind TEXT,
                projection_unit_ordinal INTEGER,
                content_digest TEXT NOT NULL,
                backend_version TEXT NOT NULL,
                sync_status TEXT NOT NULL,
                sync_error TEXT,
                synced_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_semantic_memory_links_source
            ON semantic_memory_links (notebook_id, source_id);",
        )?;
        set_schema_version(conn, NOTEBOOK_SCHEMA_VERSION)?;
    }

    if version < 4 {
        ensure_semantic_memory_projection_status(conn)?;
        set_schema_version(conn, NOTEBOOK_SCHEMA_VERSION)?;
    }

    if version < 5 {
        ensure_provenance_receipts(conn)?;
        set_schema_version(conn, NOTEBOOK_SCHEMA_VERSION)?;
    }

    if version < 6 {
        ensure_source_processing_state(conn)?;
        set_schema_version(conn, NOTEBOOK_SCHEMA_VERSION)?;
    }

    if version < 7 {
        ensure_embedding_index_metadata(conn)?;
        set_schema_version(conn, NOTEBOOK_SCHEMA_VERSION)?;
    }

    ensure_notebook_fts(conn)?;
    ensure_source_processing_state(conn)?;
    ensure_embedding_index_metadata(conn)?;
    ensure_semantic_memory_links(conn)?;
    ensure_semantic_memory_projection_status(conn)?;
    ensure_semantic_memory_vector_artifact_receipts(conn)?;
    ensure_semantic_memory_retrieval_probe_receipts(conn)?;
    ensure_prompt_generation_receipts(conn)?;
    ensure_chat_attempts(conn)?;
    ensure_provenance_receipts(conn)?;
    ensure_studio_outputs(conn)?;
    ensure_studio_outputs_prose_column(conn)?;

    Ok(())
}

fn ensure_semantic_memory_links(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS semantic_memory_links (
            chunk_id TEXT PRIMARY KEY,
            notebook_id TEXT,
            source_id TEXT,
            sm_document_id TEXT,
            sm_chunk_id TEXT,
            sm_episode_id TEXT,
            gloss_chunk_id TEXT,
            projection_unit_id TEXT,
            projection_unit_kind TEXT,
            projection_unit_ordinal INTEGER,
            content_digest TEXT,
            backend_version TEXT,
            sync_status TEXT,
            sync_error TEXT,
            synced_at TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_semantic_memory_links_source
        ON semantic_memory_links (notebook_id, source_id);",
    )?;

    for (column, definition) in [
        ("notebook_id", "TEXT"),
        ("source_id", "TEXT"),
        ("sm_document_id", "TEXT"),
        ("sm_chunk_id", "TEXT"),
        ("sm_episode_id", "TEXT"),
        ("gloss_chunk_id", "TEXT"),
        ("projection_unit_id", "TEXT"),
        ("projection_unit_kind", "TEXT"),
        ("projection_unit_ordinal", "INTEGER"),
        ("content_digest", "TEXT"),
        ("backend_version", "TEXT"),
        ("sync_status", "TEXT"),
        ("sync_error", "TEXT"),
        ("synced_at", "TEXT"),
    ] {
        if !table_has_column(conn, "semantic_memory_links", column)? {
            conn.execute(
                &format!("ALTER TABLE semantic_memory_links ADD COLUMN {column} {definition}"),
                [],
            )?;
        }
    }

    backfill_semantic_memory_content_digests(conn)?;
    conn.execute(
        "UPDATE semantic_memory_links
         SET gloss_chunk_id = COALESCE(gloss_chunk_id, chunk_id),
             projection_unit_id = COALESCE(projection_unit_id, chunk_id),
             projection_unit_kind = COALESCE(projection_unit_kind, 'chunk')
         WHERE gloss_chunk_id IS NULL
            OR projection_unit_id IS NULL
            OR projection_unit_kind IS NULL",
        [],
    )?;
    conn.execute(
        "UPDATE semantic_memory_links
         SET sync_status = 'degraded-missing-exact-backpointer',
             sync_error = COALESCE(sync_error, 'missing exact semantic chunk identity'),
             synced_at = COALESCE(synced_at, datetime('now'))
         WHERE sync_status = 'synced'
           AND (sm_chunk_id IS NULL OR trim(sm_chunk_id) = '')",
        [],
    )?;

    Ok(())
}

fn ensure_embedding_index_metadata(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS embedding_index_metadata (
            index_id TEXT PRIMARY KEY,
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            model_digest TEXT,
            dimensions INTEGER,
            schema_version INTEGER NOT NULL,
            status TEXT NOT NULL,
            status_reason TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            validated_at TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_embedding_index_metadata_status
        ON embedding_index_metadata (status);",
    )?;

    for (column, definition) in [
        ("index_id", "TEXT"),
        ("provider", "TEXT NOT NULL DEFAULT ''"),
        ("model", "TEXT NOT NULL DEFAULT ''"),
        ("model_digest", "TEXT"),
        ("dimensions", "INTEGER"),
        ("schema_version", "INTEGER NOT NULL DEFAULT 1"),
        ("status", "TEXT NOT NULL DEFAULT 'unknown'"),
        ("status_reason", "TEXT"),
        ("created_at", "TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP"),
        ("validated_at", "TEXT"),
    ] {
        if !table_has_column(conn, "embedding_index_metadata", column)? {
            conn.execute(
                &format!("ALTER TABLE embedding_index_metadata ADD COLUMN {column} {definition}"),
                [],
            )?;
        }
    }

    Ok(())
}

fn ensure_semantic_memory_projection_status(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS semantic_memory_projection_status (
            notebook_id TEXT NOT NULL,
            source_id TEXT NOT NULL,
            status TEXT NOT NULL,
            chunk_count INTEGER NOT NULL DEFAULT 0,
            projected_chunk_count INTEGER NOT NULL DEFAULT 0,
            healthy_link_count INTEGER NOT NULL DEFAULT 0,
            degraded_link_count INTEGER NOT NULL DEFAULT 0,
            last_receipt_id TEXT,
            last_error TEXT,
            artifact_generation_id TEXT,
            vector_artifact_manifest_digest TEXT,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (notebook_id, source_id),
            FOREIGN KEY(source_id) REFERENCES sources(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_semantic_memory_projection_status_status
        ON semantic_memory_projection_status (notebook_id, status);",
    )?;

    for (column, definition) in [
        ("notebook_id", "TEXT"),
        ("source_id", "TEXT"),
        ("status", "TEXT"),
        ("chunk_count", "INTEGER NOT NULL DEFAULT 0"),
        ("projected_chunk_count", "INTEGER NOT NULL DEFAULT 0"),
        ("healthy_link_count", "INTEGER NOT NULL DEFAULT 0"),
        ("degraded_link_count", "INTEGER NOT NULL DEFAULT 0"),
        ("last_receipt_id", "TEXT"),
        ("last_error", "TEXT"),
        ("artifact_generation_id", "TEXT"),
        ("vector_artifact_manifest_digest", "TEXT"),
        ("updated_at", "TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP"),
    ] {
        if !table_has_column(conn, "semantic_memory_projection_status", column)? {
            conn.execute(
                &format!(
                    "ALTER TABLE semantic_memory_projection_status ADD COLUMN {column} {definition}"
                ),
                [],
            )?;
        }
    }

    Ok(())
}

fn ensure_semantic_memory_vector_artifact_receipts(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS semantic_memory_vector_artifact_receipts (
            receipt_id TEXT PRIMARY KEY,
            notebook_id TEXT NOT NULL,
            generation_id TEXT,
            artifact_manifest_digest TEXT,
            raw_receipt_json TEXT NOT NULL,
            status TEXT NOT NULL,
            recorded_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE INDEX IF NOT EXISTS idx_semantic_memory_vector_artifact_receipts_notebook
        ON semantic_memory_vector_artifact_receipts (notebook_id, recorded_at DESC);",
    )
}

fn ensure_semantic_memory_retrieval_probe_receipts(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS semantic_memory_retrieval_probe_receipts (
            receipt_id TEXT PRIMARY KEY,
            notebook_id TEXT NOT NULL,
            query_digest TEXT NOT NULL,
            source_scope_kind TEXT NOT NULL,
            scoped_sources INTEGER NOT NULL DEFAULT 0,
            scoped_chunks INTEGER NOT NULL DEFAULT 0,
            backend_requested TEXT NOT NULL,
            backend_used TEXT NOT NULL,
            bm25_candidates INTEGER NOT NULL DEFAULT 0,
            vector_candidates INTEGER NOT NULL DEFAULT 0,
            tq_candidates INTEGER NOT NULL DEFAULT 0,
            candidate_backend TEXT,
            artifact_generation_id TEXT,
            vector_artifact_manifest_digest TEXT,
            exact_rerank INTEGER NOT NULL DEFAULT 0 CHECK (exact_rerank IN (0, 1)),
            exact_rerank_count INTEGER NOT NULL DEFAULT 0,
            fallback_used INTEGER NOT NULL DEFAULT 0 CHECK (fallback_used IN (0, 1)),
            fallback_reason TEXT,
            degradation_markers TEXT NOT NULL DEFAULT '[]',
            raw_receipt_json TEXT NOT NULL,
            recorded_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE INDEX IF NOT EXISTS idx_semantic_memory_retrieval_probe_receipts_notebook
        ON semantic_memory_retrieval_probe_receipts (notebook_id, recorded_at DESC);

        CREATE INDEX IF NOT EXISTS idx_semantic_memory_retrieval_probe_receipts_candidate
        ON semantic_memory_retrieval_probe_receipts (notebook_id, candidate_backend, exact_rerank);",
    )
}

fn ensure_provenance_receipts(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS provenance_receipts (
            receipt_id TEXT PRIMARY KEY,
            operator_kind TEXT NOT NULL,
            subject_kind TEXT NOT NULL,
            subject_id TEXT NOT NULL,
            valid_time_start TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            valid_time_end TEXT,
            recorded_time TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            supersedes_receipt_id TEXT,
            invalidated_by_receipt_id TEXT,
            raw_receipt_json TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_provenance_receipts_subject
        ON provenance_receipts (subject_kind, subject_id, recorded_time DESC);",
    )
}

fn ensure_prompt_generation_receipts(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS prompt_receipts (
            receipt_id TEXT PRIMARY KEY,
            notebook_id TEXT NOT NULL,
            conversation_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            prompt_digest TEXT NOT NULL,
            context_payload_digest TEXT NOT NULL,
            raw_receipt_json TEXT NOT NULL,
            recorded_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE INDEX IF NOT EXISTS idx_prompt_receipts_message
        ON prompt_receipts (message_id, recorded_at DESC);

        CREATE TABLE IF NOT EXISTS generation_receipts (
            receipt_id TEXT PRIMARY KEY,
            notebook_id TEXT NOT NULL,
            conversation_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            provider_request_digest TEXT NOT NULL,
            response_digest TEXT,
            status TEXT NOT NULL,
            raw_receipt_json TEXT NOT NULL,
            recorded_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE INDEX IF NOT EXISTS idx_generation_receipts_message
        ON generation_receipts (message_id, recorded_at DESC);",
    )
}

fn ensure_chat_attempts(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chat_attempts (
            attempt_id TEXT PRIMARY KEY,
            notebook_id TEXT NOT NULL,
            conversation_id TEXT NOT NULL,
            assistant_message_id TEXT NOT NULL,
            user_message_id TEXT,
            provider TEXT,
            model TEXT,
            status TEXT NOT NULL,
            phase TEXT,
            error_code TEXT,
            error_message TEXT,
            request_digest TEXT,
            response_digest TEXT,
            partial_policy TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            terminal_at TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_chat_attempts_conversation
        ON chat_attempts (conversation_id, updated_at DESC);

        CREATE INDEX IF NOT EXISTS idx_chat_attempts_assistant_message
        ON chat_attempts (assistant_message_id);",
    )
}

fn ensure_source_processing_state(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS source_processing_state (
            source_id TEXT PRIMARY KEY REFERENCES sources(id) ON DELETE CASCADE,
            lifecycle_status TEXT NOT NULL DEFAULT 'pending',
            summary_status TEXT NOT NULL DEFAULT 'missing',
            fts_index_status TEXT NOT NULL DEFAULT 'missing',
            dense_index_status TEXT NOT NULL DEFAULT 'missing',
            semantic_projection_status TEXT NOT NULL DEFAULT 'disabled',
            last_summary_receipt_id TEXT,
            last_dense_index_receipt_id TEXT,
            last_projection_receipt_id TEXT,
            last_error TEXT,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE INDEX IF NOT EXISTS idx_source_processing_state_lifecycle
        ON source_processing_state (lifecycle_status);

        CREATE INDEX IF NOT EXISTS idx_source_processing_state_dense
        ON source_processing_state (dense_index_status);

        CREATE INDEX IF NOT EXISTS idx_source_processing_state_projection
        ON source_processing_state (semantic_projection_status);",
    )?;

    for (column, definition) in [
        ("source_id", "TEXT"),
        ("lifecycle_status", "TEXT NOT NULL DEFAULT 'pending'"),
        ("summary_status", "TEXT NOT NULL DEFAULT 'missing'"),
        ("fts_index_status", "TEXT NOT NULL DEFAULT 'missing'"),
        ("dense_index_status", "TEXT NOT NULL DEFAULT 'missing'"),
        (
            "semantic_projection_status",
            "TEXT NOT NULL DEFAULT 'disabled'",
        ),
        ("last_summary_receipt_id", "TEXT"),
        ("last_dense_index_receipt_id", "TEXT"),
        ("last_projection_receipt_id", "TEXT"),
        ("last_error", "TEXT"),
        ("updated_at", "TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP"),
    ] {
        if !table_has_column(conn, "source_processing_state", column)? {
            conn.execute(
                &format!("ALTER TABLE source_processing_state ADD COLUMN {column} {definition}"),
                [],
            )?;
        }
    }

    conn.execute_batch(
        "INSERT OR IGNORE INTO source_processing_state (
            source_id,
            lifecycle_status,
            summary_status,
            fts_index_status,
            dense_index_status,
            semantic_projection_status,
            last_error,
            updated_at
        )
        SELECT
            s.id,
            COALESCE(NULLIF(s.status, ''), 'pending'),
            CASE WHEN s.summary IS NULL OR trim(s.summary) = '' THEN 'missing' ELSE 'ready' END,
            CASE WHEN EXISTS (SELECT 1 FROM chunks c WHERE c.source_id = s.id) THEN 'indexed' ELSE 'missing' END,
            CASE
                WHEN EXISTS (SELECT 1 FROM chunks c WHERE c.source_id = s.id AND c.embedding_id IS NOT NULL) THEN 'indexed'
                WHEN EXISTS (SELECT 1 FROM chunks c WHERE c.source_id = s.id) THEN 'missing'
                ELSE 'missing'
            END,
            COALESCE((
                SELECT ps.status
                FROM semantic_memory_projection_status ps
                WHERE ps.source_id = s.id
                ORDER BY ps.updated_at DESC
                LIMIT 1
            ), 'disabled'),
            s.error_message,
            datetime('now')
        FROM sources s;",
    )?;

    Ok(())
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn backfill_semantic_memory_content_digests(conn: &Connection) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "SELECT l.chunk_id, c.content
         FROM semantic_memory_links l
         JOIN chunks c ON c.id = l.chunk_id
         WHERE l.content_digest IS NULL OR trim(l.content_digest) = ''",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    for (chunk_id, content) in rows {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let digest = format!("{:x}", hasher.finalize());
        conn.execute(
            "UPDATE semantic_memory_links SET content_digest = ?1 WHERE chunk_id = ?2",
            rusqlite::params![digest, chunk_id],
        )?;
    }

    Ok(())
}

fn ensure_notebook_fts(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
            content,
            content='chunks',
            content_rowid='rowid'
        );

        CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
            INSERT INTO chunks_fts(rowid, content) VALUES (new.rowid, new.content);
        END;

        CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
            INSERT INTO chunks_fts(chunks_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
        END;

        CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE OF content ON chunks BEGIN
            INSERT INTO chunks_fts(chunks_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
            INSERT INTO chunks_fts(rowid, content) VALUES (new.rowid, new.content);
        END;

        ",
    )?;
    conn.execute(
        "INSERT INTO chunks_fts(chunks_fts) VALUES (?1)",
        ["rebuild"],
    )?;
    Ok(())
}

fn get_schema_version(conn: &Connection) -> rusqlite::Result<i32> {
    let has_meta: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_meta' COLLATE NOCASE)",
        [],
        |row| row.get(0),
    )?;
    if !has_meta {
        return Ok(0);
    }

    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM _meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    match value {
        Some(value) => value.parse::<i32>().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        }),
        None => Ok(0),
    }
}

/// Validate schema ownership with read-only queries before connection runtime
/// pragmas or migrations.
pub(crate) fn validate_schema_version(
    conn: &Connection,
    schema: DatabaseSchema,
) -> rusqlite::Result<i32> {
    let (database, supported_version) = match schema {
        DatabaseSchema::App => ("app", APP_SCHEMA_VERSION),
        DatabaseSchema::Notebook => ("notebook", NOTEBOOK_SCHEMA_VERSION),
    };
    let version = get_schema_version(conn)?;
    if version < 0 || version > supported_version {
        let message = if version < 0 {
            format!(
                "Invalid {database} database schema version {version}; expected a non-negative version"
            )
        } else {
            format!(
                "Unsupported {database} database schema version {version}; this build supports up to {supported_version}"
            )
        };
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
            Some(message),
        ));
    }
    Ok(version)
}

fn set_schema_version(conn: &Connection, version: i32) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO _meta (key, value) VALUES ('schema_version', ?1)",
        [version.to_string()],
    )?;
    Ok(())
}

fn ensure_studio_outputs(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS studio_outputs (
            id            TEXT PRIMARY KEY,
            output_type   TEXT NOT NULL,
            title         TEXT,
            prompt_used   TEXT NOT NULL,
            raw_content   TEXT,
            config        TEXT,
            source_ids    TEXT,
            file_path     TEXT,
            status        TEXT DEFAULT 'pending',
            error_message TEXT,
            prose_content TEXT,
            created_at    TEXT DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_studio_type ON studio_outputs(output_type, created_at DESC);",
    )?;
    Ok(())
}

fn ensure_studio_outputs_prose_column(conn: &Connection) -> rusqlite::Result<()> {
    if !table_has_column(conn, "studio_outputs", "prose_content")? {
        conn.execute(
            "ALTER TABLE studio_outputs ADD COLUMN prose_content TEXT",
            [],
        )?;
    }
    Ok(())
}

/// Synthetic crash-state fixtures shared by the public admission and startup
/// tests. The producer stays open while its main/WAL/SHM files are copied;
/// otherwise SQLite would checkpoint the very frames under test on close.
#[cfg(test)]
pub(crate) mod wal_admission_fixtures {
    use super::*;
    use std::path::{Path, PathBuf};

    pub(crate) struct CopiedWal {
        _producer: Connection,
        _producer_directory: tempfile::TempDir,
    }

    pub(crate) fn sidecar(path: &Path, suffix: &str) -> PathBuf {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        PathBuf::from(name)
    }

    pub(crate) fn file_state(path: &Path) -> [Option<Vec<u8>>; 4] {
        ["", "-wal", "-shm", "-journal"].map(|suffix| {
            let path = sidecar(path, suffix);
            match std::fs::read(&path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => panic!(
                    "cannot inspect synthetic fixture {}: {error}",
                    path.display()
                ),
            }
        })
    }

    pub(crate) fn record_file_state(label: &str, state: &[Option<Vec<u8>>; 4]) {
        let Some(root) = std::env::var_os("GLOSS_WAL_EVIDENCE_DIR") else {
            return;
        };
        let folder = PathBuf::from(root).join(label);
        std::fs::create_dir_all(&folder).unwrap();
        let files: Vec<_> = ["", "-wal", "-shm", "-journal"]
            .into_iter()
            .zip(state)
            .map(|(suffix, bytes)| {
                if let Some(bytes) = bytes {
                    std::fs::write(folder.join(format!("fixture.db{suffix}")), bytes).unwrap();
                }
                serde_json::json!({
                    "suffix": suffix,
                    "exists": bytes.is_some(),
                    "size": bytes.as_ref().map(Vec::len),
                    "sha256": bytes.as_ref().map(|bytes| format!("{:x}", Sha256::digest(bytes)))
                })
            })
            .collect();
        let record = serde_json::json!({"label": label, "files": files});
        std::fs::write(
            folder.join("state.json"),
            serde_json::to_vec_pretty(&record).unwrap(),
        )
        .unwrap();
        println!("WAL_FILE_STATE {record}");
    }

    impl CopiedWal {
        pub(crate) fn create(
            copied: &Path,
            schema: DatabaseSchema,
            version: &str,
            include_shm: bool,
        ) -> Self {
            let producer_directory = tempfile::tempdir().unwrap();
            let original = producer_directory.path().join("producer.db");
            let producer = Connection::open(&original).unwrap();
            match schema {
                DatabaseSchema::App => migrate_app_db(&producer).unwrap(),
                DatabaseSchema::Notebook => migrate_notebook_db(&producer).unwrap(),
            }
            producer
                .execute_batch("PRAGMA wal_autocheckpoint=0;")
                .unwrap();
            producer
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .unwrap();
            let checkpointed_main = std::fs::read(&original).unwrap();
            let transaction = producer.unchecked_transaction().unwrap();
            transaction
                .execute_batch(
                    "CREATE TABLE wal_retained(value TEXT NOT NULL);
                 INSERT INTO wal_retained VALUES ('committed WAL record');",
                )
                .unwrap();
            transaction
                .execute(
                    "UPDATE _meta SET value=?1 WHERE key='schema_version'",
                    [version],
                )
                .unwrap();
            transaction.commit().unwrap();
            assert_eq!(
                std::fs::read(&original).unwrap(),
                checkpointed_main,
                "the rejecting version and retained record must remain in WAL"
            );
            assert!(std::fs::metadata(sidecar(&original, "-wal")).unwrap().len() > 32);
            for suffix in ["", "-wal"] {
                std::fs::copy(sidecar(&original, suffix), sidecar(copied, suffix)).unwrap();
            }
            if include_shm {
                std::fs::copy(sidecar(&original, "-shm"), sidecar(copied, "-shm")).unwrap();
            }
            Self {
                _producer: producer,
                _producer_directory: producer_directory,
            }
        }

        /// Verify logical readability on another disposable copy, so this
        /// assertion cannot itself checkpoint or alter the rejected triplet.
        pub(crate) fn assert_readable(path: &Path, version: &str) {
            let directory = tempfile::tempdir().unwrap();
            let proof = directory.path().join("read-proof.db");
            for suffix in ["", "-wal", "-shm"] {
                let source = sidecar(path, suffix);
                if source.exists() {
                    std::fs::copy(source, sidecar(&proof, suffix)).unwrap();
                }
            }
            let reader = Connection::open(&proof).unwrap();
            assert_eq!(
                reader
                    .query_row(
                        "SELECT value FROM _meta WHERE key='schema_version'",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .unwrap(),
                version
            );
            assert_eq!(
                reader
                    .query_row("SELECT value FROM wal_retained", [], |row| row
                        .get::<_, String>(0),)
                    .unwrap(),
                "committed WAL record"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::app_db::AppDb;
    use crate::db::notebook_db::NotebookDb;
    use crate::db::notebook_pool::NotebookDbPool;
    use crate::error::GlossError;
    use std::path::Path;
    use tempfile::tempdir;

    type GuardedOpen = fn(&Path) -> Result<(), GlossError>;

    fn schema_guard_entrypoints() -> [(&'static str, i32, GuardedOpen); 5] {
        [
            ("app_open", APP_SCHEMA_VERSION, |path| {
                AppDb::open(path).map(|_| ())
            }),
            ("notebook_open", NOTEBOOK_SCHEMA_VERSION, |path| {
                NotebookDb::open(path).map(|_| ())
            }),
            ("notebook_connect", NOTEBOOK_SCHEMA_VERSION, |path| {
                NotebookDb::connect(path).map(|_| ())
            }),
            ("pool_new", NOTEBOOK_SCHEMA_VERSION, |path| {
                NotebookDbPool::new(path).map(|_| ())
            }),
            ("pool_custom", NOTEBOOK_SCHEMA_VERSION, |path| {
                NotebookDbPool::with_max_read_conns(path, 2).map(|_| ())
            }),
        ]
    }

    #[test]
    fn copied_committed_wal_rejection_preserves_main_wal_shm_bytes_and_existence() {
        use super::wal_admission_fixtures::{file_state, record_file_state, CopiedWal};
        for (entrypoint, supported_version, open) in schema_guard_entrypoints() {
            for version in [
                (supported_version + 1).to_string(),
                "malformed".into(),
                "-1".into(),
            ] {
                for include_shm in [true, false] {
                    let directory = tempdir().unwrap();
                    let path = directory.path().join("rejected.db");
                    let schema = if entrypoint == "app_open" {
                        DatabaseSchema::App
                    } else {
                        DatabaseSchema::Notebook
                    };
                    let _fixture = CopiedWal::create(&path, schema, &version, include_shm);
                    let before = file_state(&path);
                    let case = format!("public-{entrypoint}-{version}-shm-{include_shm}");
                    record_file_state(&format!("{case}-before"), &before);
                    CopiedWal::assert_readable(&path, &version);
                    let result = open(&path);
                    record_file_state(&format!("{case}-after"), &file_state(&path));
                    println!("WAL_ADMISSION {case} error={:?}", result.as_ref().err());
                    assert!(result.is_err(), "{entrypoint} admitted schema {version}");
                    assert_eq!(
                        file_state(&path),
                        before,
                        "{entrypoint} changed rejected schema {version}, SHM present={include_shm}"
                    );
                    CopiedWal::assert_readable(&path, &version);
                    assert_eq!(
                        file_state(&path),
                        before,
                        "readability proof changed source fixture"
                    );
                }
            }
        }
    }

    #[test]
    fn bundled_sqlite_runtime_identity() {
        let conn = Connection::open_in_memory().unwrap();
        let version: String = conn
            .query_row("SELECT sqlite_version()", [], |r| r.get(0))
            .unwrap();
        let source_id: String = conn
            .query_row("SELECT sqlite_source_id()", [], |r| r.get(0))
            .unwrap();
        let options = conn
            .prepare("PRAGMA compile_options")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        println!(
            "SQLITE_RUNTIME {}",
            serde_json::json!({"version":version,"source_id":source_id,"compile_options":options,"rusqlite":"0.32.1","libsqlite3_sys":"0.30.1"})
        );
        assert_eq!(version, "3.46.0");
        assert!(options.iter().any(|v| v == "ENABLE_FTS5"));
    }

    #[test]
    fn supported_committed_wal_owner_writes_survive_cold_reopen() {
        use super::wal_admission_fixtures::CopiedWal;
        for (entrypoint, supported, _) in schema_guard_entrypoints() {
            for include_shm in [true, false] {
                let directory = tempdir().unwrap();
                let path = directory.path().join("supported-write.db");
                let schema = if entrypoint == "app_open" {
                    DatabaseSchema::App
                } else {
                    DatabaseSchema::Notebook
                };
                let _fixture =
                    CopiedWal::create(&path, schema, &supported.to_string(), include_shm);
                match entrypoint {
                    "app_open" => {
                        let db = AppDb::open(&path).unwrap();
                        db.conn()
                            .execute("INSERT INTO wal_retained VALUES ('owner write')", [])
                            .unwrap();
                    }
                    "notebook_open" => {
                        let db = NotebookDb::open(&path).unwrap();
                        db.conn()
                            .execute("INSERT INTO wal_retained VALUES ('owner write')", [])
                            .unwrap();
                    }
                    "notebook_connect" => {
                        let db = NotebookDb::connect(&path).unwrap();
                        db.conn()
                            .execute("INSERT INTO wal_retained VALUES ('owner write')", [])
                            .unwrap();
                    }
                    _ => {
                        let pool = if entrypoint == "pool_new" {
                            NotebookDbPool::new(&path).unwrap()
                        } else {
                            NotebookDbPool::with_max_read_conns(&path, 2).unwrap()
                        };
                        pool.write(|db| {
                            db.conn()
                                .execute("INSERT INTO wal_retained VALUES ('owner write')", [])?;
                            Ok(())
                        })
                        .unwrap();
                    }
                }
                let conn = open_connection(&path, schema).unwrap();
                let values = conn
                    .prepare("SELECT value FROM wal_retained ORDER BY rowid")
                    .unwrap()
                    .query_map([], |r| r.get::<_, String>(0))
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                assert_eq!(values, ["committed WAL record", "owner write"]);
                println!("WAL_OWNER_REOPEN {entrypoint} SHM={include_shm} version={supported} records={values:?}");
            }
        }
    }

    fn live_wal_writer(path: &Path, schema: DatabaseSchema, version: &str) -> Connection {
        let writer = Connection::open(path).unwrap();
        match schema {
            DatabaseSchema::App => migrate_app_db(&writer).unwrap(),
            DatabaseSchema::Notebook => migrate_notebook_db(&writer).unwrap(),
        }
        writer.execute_batch("PRAGMA wal_autocheckpoint=0; CREATE TABLE wal_retained(value TEXT); PRAGMA wal_checkpoint(TRUNCATE); BEGIN IMMEDIATE;").unwrap();
        writer
            .execute(
                "UPDATE _meta SET value=?1 WHERE key='schema_version'",
                [version],
            )
            .unwrap();
        writer
            .execute_batch("INSERT INTO wal_retained VALUES ('committed WAL record'); COMMIT;")
            .unwrap();
        writer
    }

    #[test]
    fn active_writer_rejection_preserves_store_triplet_until_barrier_release() {
        use super::wal_admission_fixtures::{file_state, record_file_state, CopiedWal};
        use std::sync::{Arc, Barrier};
        for (entrypoint, supported, open) in schema_guard_entrypoints() {
            for version in [(supported + 1).to_string(), "malformed".into(), "-1".into()] {
                let directory = tempdir().unwrap();
                let path = directory.path().join("held-writer.db");
                let schema = if entrypoint == "app_open" {
                    DatabaseSchema::App
                } else {
                    DatabaseSchema::Notebook
                };
                let writer = live_wal_writer(&path, schema, &version);
                let entered = Arc::new(Barrier::new(2));
                let release = Arc::new(Barrier::new(2));
                let (writer_entered, writer_release) = (entered.clone(), release.clone());
                let worker = std::thread::spawn(move || {
                    writer.execute_batch("BEGIN IMMEDIATE; INSERT INTO wal_retained VALUES ('writer commit after release');").unwrap();
                    writer_entered.wait();
                    writer_release.wait();
                    writer.execute_batch("COMMIT;").unwrap();
                });
                entered.wait();
                let before = file_state(&path);
                let label = format!("held-{entrypoint}-{version}");
                record_file_state(&format!("{label}-before"), &before);
                let result = open(&path);
                let after = file_state(&path);
                record_file_state(&format!("{label}-after"), &after);
                release.wait();
                worker.join().unwrap();
                assert!(result.is_err(), "{entrypoint} admitted {version}");
                assert_eq!(
                    before, after,
                    "{label} changed source while writer was held"
                );
                CopiedWal::assert_readable(&path, &version);
                let copy = tempdir().unwrap();
                let proof = copy.path().join("proof.db");
                for suffix in ["", "-wal", "-shm"] {
                    let from = super::wal_admission_fixtures::sidecar(&path, suffix);
                    if from.exists() {
                        std::fs::copy(from, super::wal_admission_fixtures::sidecar(&proof, suffix))
                            .unwrap();
                    }
                }
                let reader = Connection::open(proof).unwrap();
                assert_eq!(
                    reader
                        .query_row("SELECT COUNT(*) FROM wal_retained", [], |r| r
                            .get::<_, i64>(0))
                        .unwrap(),
                    2
                );
            }
        }
    }

    #[test]
    fn supported_admission_reads_committed_view_with_a_barrier_held_writer() {
        use std::sync::{Arc, Barrier};
        let directory = tempdir().unwrap();
        let path = directory.path().join("supported-held.db");
        let writer = live_wal_writer(&path, DatabaseSchema::Notebook, "7");
        let pool = NotebookDbPool::new(&path).unwrap();
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let (writer_entered, writer_release) = (entered.clone(), release.clone());
        let worker = std::thread::spawn(move || {
            writer.execute_batch("BEGIN IMMEDIATE; INSERT INTO wal_retained VALUES ('writer committed after release');").unwrap();
            writer_entered.wait();
            writer_release.wait();
            writer.execute_batch("COMMIT").unwrap();
        });
        entered.wait();
        let db = NotebookDb::connect(&path).unwrap();
        assert_eq!(
            db.conn()
                .query_row("SELECT COUNT(*) FROM wal_retained", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        pool.read(|db| {
            assert_eq!(
                db.conn()
                    .query_row("SELECT COUNT(*) FROM wal_retained", [], |r| r
                        .get::<_, i64>(0))?,
                1
            );
            Ok(())
        })
        .unwrap();
        release.wait();
        worker.join().unwrap();
        pool.write(|db| {
            db.conn().execute(
                "INSERT INTO wal_retained VALUES ('pool still writable')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        drop(db);
        drop(pool);
        let reopened = NotebookDb::open(&path).unwrap();
        assert_eq!(
            reopened
                .conn()
                .query_row("SELECT COUNT(*) FROM wal_retained", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            3
        );
    }

    #[test]
    fn admission_between_controlled_commits_rejects_new_future_schema_without_mutation() {
        use super::wal_admission_fixtures::{file_state, record_file_state};
        use std::sync::{Arc, Barrier};
        let directory = tempdir().unwrap();
        let path = directory.path().join("between-commits.db");
        let writer = live_wal_writer(&path, DatabaseSchema::Notebook, "7");
        let phase = Arc::new(Barrier::new(2));
        let writer_phase = phase.clone();
        let worker = std::thread::spawn(move || {
            writer_phase.wait();
            writer_phase.wait();
            writer
                .execute("UPDATE _meta SET value='8' WHERE key='schema_version'", [])
                .unwrap();
            writer_phase.wait();
            writer_phase.wait();
            writer
                .execute(
                    "INSERT INTO wal_retained VALUES ('writer remains usable')",
                    [],
                )
                .unwrap();
        });
        phase.wait();
        let supported = NotebookDb::connect(&path).unwrap();
        assert_eq!(
            supported
                .conn()
                .query_row("SELECT COUNT(*) FROM wal_retained", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(supported);
        phase.wait();
        phase.wait();
        let before = file_state(&path);
        record_file_state("between-commits-before", &before);
        let result = NotebookDb::connect(&path);
        let after = file_state(&path);
        record_file_state("between-commits-after", &after);
        phase.wait();
        worker.join().unwrap();
        assert!(
            matches!(result,Err(GlossError::Database(rusqlite::Error::SqliteFailure(_,Some(ref message)))) if message.contains("schema version 8"))
        );
        assert_eq!(before, after);
    }

    #[test]
    fn source_change_during_inspection_fails_closed_without_source_acquisition() {
        use super::wal_admission_fixtures::{file_state, record_file_state};
        use std::cell::RefCell;
        use std::rc::Rc;
        let directory = tempdir().unwrap();
        let path = directory.path().join("inspection-transition.db");
        let writer = Rc::new(live_wal_writer(&path, DatabaseSchema::Notebook, "7"));
        let _keep_writer = writer.clone();
        let changed = Rc::new(RefCell::new(None));
        let hook_changed = changed.clone();
        let hook_path = path.clone();
        AFTER_INSPECTION_COPY.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                writer
                    .execute("UPDATE _meta SET value='8' WHERE key='schema_version'", [])
                    .unwrap();
                let state = file_state(&hook_path);
                record_file_state("inspection-transition-actor-commit", &state);
                *hook_changed.borrow_mut() = Some(state);
            }));
        });
        let error = match NotebookDb::open(&path) {
            Ok(_) => panic!("changing source admitted"),
            Err(error) => error,
        };
        assert!(
            matches!(error,GlossError::Database(rusqlite::Error::SqliteFailure(ref code,Some(ref message))) if code.extended_code==rusqlite::ffi::SQLITE_BUSY && message.contains("changed during schema inspection"))
        );
        let after = file_state(&path);
        record_file_state("inspection-transition-after", &after);
        assert_eq!(changed.borrow().as_ref().unwrap(), &after);
    }

    #[test]
    fn schema_transition_real_handle_refuses_newer_schema_before_migration() {
        use super::wal_admission_fixtures::{file_state, record_file_state};
        use std::cell::RefCell;
        use std::rc::Rc;
        let directory = tempdir().unwrap();
        let path = directory.path().join("acquisition-transition.db");
        let writer = Rc::new(live_wal_writer(&path, DatabaseSchema::Notebook, "7"));
        let kept_writer = writer.clone();
        let changed = Rc::new(RefCell::new(None));
        let hook_changed = changed.clone();
        let hook_path = path.clone();
        BEFORE_SOURCE_ACQUISITION.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                writer
                    .execute("UPDATE _meta SET value='8' WHERE key='schema_version'", [])
                    .unwrap();
                let state = file_state(&hook_path);
                record_file_state("acquisition-transition-actor-commit", &state);
                *hook_changed.borrow_mut() = Some(state);
            }));
        });
        let error = match NotebookDb::open(&path) {
            Ok(_) => panic!("new future schema migrated"),
            Err(error) => error,
        };
        assert!(
            matches!(error,GlossError::Database(rusqlite::Error::SqliteFailure(_,Some(ref message))) if message.contains("schema version 8"))
        );
        let after = file_state(&path);
        record_file_state("acquisition-transition-after", &after);
        let before = changed.borrow();
        let before = before.as_ref().unwrap();
        assert_eq!(before[0], after[0], "raced rejection changed main bytes");
        assert_eq!(before[1], after[1], "raced rejection changed WAL bytes");
        let changes: Vec<_> = ["main", "wal", "shm", "journal"]
            .into_iter()
            .enumerate()
            .filter_map(|(i, name)| (before[i] != after[i]).then_some(name))
            .collect();
        println!(
            "WAL_RACED_REJECTION {}",
            serde_json::json!({"changed_files":changes,"full_preservation":before==&after,"actual_schema_rejected":8,"migration_performed":false})
        );
        assert_eq!(
            kept_writer
                .query_row(
                    "SELECT value FROM _meta WHERE key='schema_version'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "8"
        );
    }

    fn assert_raced_notebook_content(connection: &Connection, version: &str, count: i64) {
        let actual_version: String = connection
            .query_row(
                "SELECT value FROM _meta WHERE key='schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let actual_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM wal_retained", [], |row| row.get(0))
            .unwrap();
        let values = connection
            .prepare("SELECT value FROM wal_retained ORDER BY rowid")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        let expected_values: &[&str] = if count == 1 {
            &["committed WAL record"]
        } else {
            assert_eq!(count, 2, "bounded fixture has exactly two retained rows");
            &["committed WAL record", "writer durable after refusal"]
        };
        assert_eq!(actual_version, version);
        assert_eq!(actual_count, count);
        assert_eq!(values, expected_values);
    }

    fn assert_raced_notebook_integrity(connection: &Connection) {
        let diagnoses = connection
            .prepare("PRAGMA integrity_check")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(diagnoses, ["ok"]);
    }

    fn assert_raced_refusal_changes_only_reader_marks(
        before: &[Option<Vec<u8>>; 4],
        after: &[Option<Vec<u8>>; 4],
    ) -> Vec<(usize, u32, u32)> {
        for (index, name) in [(0, "main"), (1, "WAL"), (3, "journal")] {
            assert_eq!(before[index], after[index], "raced refusal changed {name}");
        }
        let before_shm = before[2].as_ref().expect("actor commit has live SHM");
        let after_shm = after[2].as_ref().expect("refusal retained live SHM");
        assert_eq!(before_shm.len(), after_shm.len(), "SHM size changed");
        assert!(before_shm.len() >= 136, "complete bundled SQLite WAL index");
        // Bundled SQLite uses native-endian u32 words. Its two 48-byte header
        // copies precede nBackfill, aReadMark[0..=4], and lock bytes at120.
        // This is a controlled characterization, never a preservation policy.
        assert_eq!(&before_shm[..48], &before_shm[48..96]);
        assert_eq!(before_shm[12], 1, "WAL index header is initialized");
        assert_eq!(&before_shm[..104], &after_shm[..104]);
        assert_eq!(&before_shm[120..], &after_shm[120..]);
        let mx_frame = u32::from_ne_bytes(before_shm[16..20].try_into().unwrap());
        assert!(mx_frame > 0, "fixture must retain committed WAL frames");
        let mut changed = Vec::new();
        for slot in 1..=4 {
            let offset = 100 + 4 * slot;
            let old = u32::from_ne_bytes(before_shm[offset..offset + 4].try_into().unwrap());
            let new = u32::from_ne_bytes(after_shm[offset..offset + 4].try_into().unwrap());
            assert!(
                old <= mx_frame || old == u32::MAX,
                "reader mark{slot} had an invalid previous frame"
            );
            if old != new {
                assert!(
                    old < mx_frame || old == u32::MAX,
                    "changed reader mark{slot} did not advance from an older frame"
                );
                assert_eq!(new, mx_frame, "reader mark{slot} did not become mxFrame");
                changed.push((slot, old, new));
            }
        }
        assert!(
            !changed.is_empty(),
            "fixture must characterize raced SHM mutation"
        );
        changed
    }

    fn assert_raced_cold_copy_retains_future_schema(path: &Path) {
        use super::wal_admission_fixtures::{file_state, sidecar};
        let before = file_state(path);
        let directory = tempdir().unwrap();
        let copied = directory.path().join("independent-cold-proof.db");
        for (index, suffix) in [(0, ""), (1, "-wal"), (3, "-journal")] {
            if before[index].is_some() {
                std::fs::copy(sidecar(path, suffix), sidecar(&copied, suffix)).unwrap();
            }
        }
        assert!(
            !sidecar(&copied, "-shm").exists(),
            "copy must reconstruct its SHM"
        );
        {
            let recovered = Connection::open(&copied).unwrap();
            assert_raced_notebook_content(&recovered, "8", 2);
            assert_raced_notebook_integrity(&recovered);
        }
        assert_eq!(file_state(path), before, "cold proof changed source files");
    }

    fn characterize_raced_notebook_durable_safety(case: &'static str, hold_reader: bool) {
        use super::wal_admission_fixtures::{file_state, record_file_state};
        use std::cell::RefCell;
        use std::rc::Rc;
        let directory = tempdir().unwrap();
        let path = directory.path().join("raced-durable-safety.db");
        let kept_writer = Rc::new(live_wal_writer(&path, DatabaseSchema::Notebook, "7"));
        let old_reader = hold_reader.then(|| {
            let reader = Connection::open_with_flags(
                &path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .unwrap();
            reader.execute_batch("BEGIN").unwrap();
            assert_raced_notebook_content(&reader, "7", 1);
            reader
        });
        let held_read_mark = old_reader.as_ref().map(|_| {
            let state = file_state(&path);
            record_file_state(&format!("{case}-held-reader-before-future-commit"), &state);
            let shm = state[2].as_ref().expect("old reader has live SHM");
            assert!(shm.len() >= 136);
            assert_eq!(&shm[..48], &shm[48..96]);
            assert_eq!(shm[12], 1);
            let frame = u32::from_ne_bytes(shm[16..20].try_into().unwrap());
            assert!(frame > 0);
            let marks = [1, 2, 3, 4].map(|slot| {
                let offset = 100 + 4 * slot;
                u32::from_ne_bytes(shm[offset..offset + 4].try_into().unwrap())
            });
            assert_eq!(marks[0], frame, "fresh old reader must hold slot1");
            assert_eq!(marks.iter().filter(|&&mark| mark == frame).count(), 1);
            (1usize, frame)
        });
        let actor_commit = Rc::new(RefCell::new(None));
        let hook_commit = Rc::clone(&actor_commit);
        let hook_writer = Rc::clone(&kept_writer);
        let hook_path = path.clone();
        BEFORE_SOURCE_ACQUISITION.with(|slot| {
            assert!(slot.borrow().is_none(), "another acquisition hook is armed");
            *slot.borrow_mut() = Some(Box::new(move || {
                hook_writer
                    .execute("UPDATE _meta SET value='8' WHERE key='schema_version'", [])
                    .unwrap();
                let before = file_state(&hook_path);
                record_file_state(&format!("{case}-actor-commit-before-acquisition"), &before);
                *hook_commit.borrow_mut() = Some(before);
                // The actor performs no further writes before source refusal.
            }));
        });
        let error = match NotebookDb::open(&path) {
            Ok(_) => panic!("new future notebook schema was admitted"),
            Err(error) => error,
        };
        let after = file_state(&path);
        record_file_state(&format!("{case}-after-refusal"), &after);
        assert!(matches!(
            error,
            GlossError::Database(rusqlite::Error::SqliteFailure(ref code, Some(ref message)))
                if code.extended_code == rusqlite::ffi::SQLITE_ERROR
                    && message.contains("Unsupported notebook database schema version 8")
                    && message.contains("supports up to 7")
        ));
        let before = actor_commit.borrow();
        let before = before.as_ref().expect("future actor commit was captured");
        if let Some((slot, frame)) = held_read_mark {
            let offset = 100 + 4 * slot;
            for state in [before, &after] {
                let shm = state[2].as_ref().expect("held reader retained SHM");
                let mark = u32::from_ne_bytes(shm[offset..offset + 4].try_into().unwrap());
                assert_eq!(mark, frame, "refusal changed held old-reader slot{slot}");
            }
        }
        let changed_marks = assert_raced_refusal_changes_only_reader_marks(before, &after);
        let full_preservation = before == &after;
        if let Some(reader) = old_reader.as_ref() {
            assert_raced_notebook_content(reader, "7", 1);
        }
        kept_writer
            .execute(
                "INSERT INTO wal_retained VALUES ('writer durable after refusal')",
                [],
            )
            .unwrap();
        assert_raced_notebook_content(&kept_writer, "8", 2);
        if let Some(reader) = old_reader.as_ref() {
            assert_raced_notebook_content(reader, "7", 1);
        }
        record_file_state(
            &format!("{case}-after-retained-writer-insert"),
            &file_state(&path),
        );
        assert_raced_cold_copy_retains_future_schema(&path);
        let mut held_checkpoint = None;
        let mut released_checkpoint = None;
        if let Some(reader) = old_reader {
            kept_writer.busy_timeout(std::time::Duration::ZERO).unwrap();
            let held: (i64, i64, i64) = kept_writer
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .unwrap();
            assert_eq!(held.0, 1, "old snapshot must block truncating checkpoint");
            assert!(
                held.1 > held.2,
                "held reader must leave frames uncheckpointed"
            );
            assert_raced_notebook_content(&reader, "7", 1);
            reader.execute_batch("ROLLBACK").unwrap();
            drop(reader);
            let released: (i64, i64, i64) = kept_writer
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .unwrap();
            assert_eq!(
                released,
                (0, 0, 0),
                "released snapshot must permit truncation"
            );
            assert_raced_notebook_content(&kept_writer, "8", 2);
            held_checkpoint = Some(held);
            released_checkpoint = Some(released);
        }
        assert_raced_notebook_integrity(&kept_writer);
        println!(
            "WAL_RACED_DURABLE_SAFETY {}",
            serde_json::json!({
                "case": case,
                "scope": "Notebook schema7 to8 controlled characterization only",
                "actual_schema_rejected": 8,
                "typed_future_refusal": true,
                "main_wal_journal_preserved": true,
                "shm_size_preserved": true,
                "allowed_shm_byte_range": [104, 120],
                "changed_reader_marks": changed_marks,
                "full_preservation": full_preservation,
                "mandatory_strict_preservation_acceptance_closed": false,
                "retained_writer_schema": 8,
                "retained_writer_count": 2,
                "cold_copy_omitted_shm": true,
                "cold_copy_schema": 8,
                "cold_copy_count": 2,
                "cold_copy_integrity_ok": true,
                "old_reader_snapshot_held": hold_reader,
                "held_reader_mark": held_read_mark,
                "held_checkpoint": held_checkpoint,
                "released_checkpoint": released_checkpoint
            })
        );
    }

    #[test]
    fn raced_future_notebook_refusal_retains_writer_and_cold_recoverable_data() {
        characterize_raced_notebook_durable_safety("raced-writer-and-cold-copy", false);
    }

    #[test]
    fn raced_future_notebook_refusal_preserves_old_reader_until_checkpoint_release() {
        characterize_raced_notebook_durable_safety("raced-old-reader-and-checkpoint", true);
    }

    #[test]
    fn physical_path_and_recovery_sidecar_edge_cases_fail_closed() {
        use super::wal_admission_fixtures::{file_state, sidecar, CopiedWal};
        let directory = tempdir().unwrap();
        let special = directory.path().join("spaces ?#% unicode-é.db");
        let _fixture = CopiedWal::create(&special, DatabaseSchema::Notebook, "8", false);
        let before = file_state(&special);
        assert!(NotebookDb::open(&special).is_err());
        assert_eq!(before, file_state(&special));
        for main in [false, true] {
            let path = directory.path().join(format!("empty-{main}.db"));
            if main {
                std::fs::write(&path, []).unwrap();
            }
            std::fs::write(sidecar(&path, "-wal"), b"retained unknown WAL").unwrap();
            let before = file_state(&path);
            assert!(
                matches!(open_connection(&path,DatabaseSchema::Notebook),Err(rusqlite::Error::SqliteFailure(ref code,_)) if code.extended_code==rusqlite::ffi::SQLITE_CORRUPT)
            );
            assert_eq!(before, file_state(&path));
        }
        let zero = directory.path().join("zero-wal.db");
        drop(NotebookDb::open(&zero).unwrap());
        std::fs::write(sidecar(&zero, "-wal"), []).unwrap();
        NotebookDb::open(&zero).unwrap();
        #[cfg(unix)]
        {
            let alias = directory.path().join("alias.db");
            std::os::unix::fs::symlink(&special, &alias).unwrap();
            let before = file_state(&special);
            assert!(NotebookDb::open(&alias).is_err());
            assert_eq!(before, file_state(&special));
        }
    }

    #[test]
    fn larger_store_admission_and_pool_connections_have_bounded_copy_cleanup() {
        use super::wal_admission_fixtures::{file_state, record_file_state, sidecar};
        use std::sync::{mpsc, Arc, Barrier};
        let directory = tempdir().unwrap();
        let retained = std::env::var_os("GLOSS_LARGE_FIXTURE_DIR").map(std::path::PathBuf::from);
        let root = retained.as_deref().unwrap_or(directory.path());
        std::fs::create_dir_all(root).unwrap();
        let path = root.join("larger-store.db");
        assert!(!path.exists(), "large fixture destination must be fresh");
        let db = NotebookDb::open(&path).unwrap();
        db.conn().execute_batch("CREATE TABLE admission_payload(id INTEGER PRIMARY KEY, value BLOB NOT NULL); BEGIN IMMEDIATE;").unwrap();
        for i in 0..256 {
            db.conn()
                .execute(
                    "INSERT INTO admission_payload VALUES (?1,zeroblob(131072))",
                    [i],
                )
                .unwrap();
        }
        db.conn()
            .execute_batch("COMMIT; PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        drop(db);
        let source_bytes: [u64; 4] = ["", "-wal", "-shm", "-journal"].map(|suffix| {
            std::fs::metadata(sidecar(&path, suffix))
                .map(|m| m.len())
                .unwrap_or(0)
        });
        assert!(source_bytes.iter().sum::<u64>() < 128 * 1024 * 1024);
        if let Some(retained) = retained.as_ref() {
            let original = retained.join("original-before-admission");
            std::fs::create_dir(&original).unwrap();
            for suffix in ["", "-wal", "-shm", "-journal"] {
                let from = sidecar(&path, suffix);
                if from.exists() {
                    std::fs::copy(from, original.join(format!("fixture.db{suffix}"))).unwrap();
                }
            }
            record_file_state("larger-original-before-admission", &file_state(&path));
        }
        let inspection_bytes = || {
            std::fs::read_dir(std::env::temp_dir())
                .unwrap()
                .filter_map(Result::ok)
                .map(|entry| {
                    ["", "-wal", "-shm", "-journal"]
                        .into_iter()
                        .map(|suffix| {
                            std::fs::metadata(
                                entry.path().join(format!("schema-inspection.db{suffix}")),
                            )
                            .map(|m| m.len())
                            .unwrap_or(0)
                        })
                        .sum::<u64>()
                })
                .sum::<u64>()
        };
        assert_eq!(inspection_bytes(), 0, "pre-existing inspection copies");
        AFTER_INSPECTION_COPY.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                println!("LARGE_INSPECTION_COPY_BYTES {}", inspection_bytes());
            }))
        });
        let start = std::time::Instant::now();
        let admitted = NotebookDb::connect(&path).unwrap();
        let direct_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(
            admitted
                .conn()
                .query_row("SELECT COUNT(*) FROM admission_payload", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            256
        );
        drop(admitted);
        assert_eq!(
            inspection_bytes(),
            0,
            "inspection copy leaked after direct admission"
        );
        let pool_start = std::time::Instant::now();
        let pool = Arc::new(NotebookDbPool::with_max_read_conns(&path, 4).unwrap());
        let pool_ms = pool_start.elapsed().as_secs_f64() * 1000.0;
        let release = Arc::new(Barrier::new(5));
        let (send, receive) = mpsc::channel();
        let mut workers = Vec::new();
        let mut read_ms = Vec::new();
        for _ in 0..4 {
            let pool = pool.clone();
            let release = release.clone();
            let send = send.clone();
            workers.push(std::thread::spawn(move || {
                let start = std::time::Instant::now();
                pool.read(|db| {
                    assert_eq!(
                        db.conn()
                            .query_row("SELECT COUNT(*) FROM admission_payload", [], |r| r
                                .get::<_, i64>(0))?,
                        256
                    );
                    send.send(start.elapsed().as_secs_f64() * 1000.0).unwrap();
                    release.wait();
                    Ok(())
                })
                .unwrap();
            }));
            read_ms.push(
                receive
                    .recv_timeout(std::time::Duration::from_secs(60))
                    .unwrap(),
            );
        }
        release.wait();
        for worker in workers {
            worker.join().unwrap();
        }
        drop(pool);
        assert_eq!(
            inspection_bytes(),
            0,
            "inspection copies leaked after pooled admission"
        );
        println!(
            "LARGE_ADMISSION_OBSERVATION {}",
            serde_json::json!({"payload_bytes":32*1024*1024,"source_file_bytes":source_bytes,"direct_connection_ms":direct_ms,"pool_constructor_ms":pool_ms,"four_distinct_lazy_reader_ms":read_ms,"inspection_bytes_after":inspection_bytes(),"proof_scope":"single bounded compatibility/cost observation; no benchmark or performance certification"})
        );
    }

    #[test]
    fn wal_admission_configuration_discrimination() {
        use super::wal_admission_fixtures::{file_state, record_file_state, sidecar, CopiedWal};
        use rusqlite::{config::DbConfig, OpenFlags};
        for mode in [
            "no-close-checkpoint",
            "readonly-shm-or-exclusive",
            "disposable-inspection",
        ] {
            for condition in [
                "copied-shm",
                "copied-no-shm",
                "live-future",
                "live-malformed",
                "live-stale-future",
                "live-stale-malformed",
            ] {
                let directory = tempdir().unwrap();
                let path = directory.path().join("probe.db");
                let version = if condition.ends_with("malformed") {
                    "malformed"
                } else {
                    "8"
                };
                let copied;
                let writer;
                if condition.starts_with("copied") {
                    copied = Some(CopiedWal::create(
                        &path,
                        DatabaseSchema::Notebook,
                        version,
                        condition == "copied-shm",
                    ));
                    writer = None;
                } else {
                    copied = None;
                    let live = Connection::open(&path).unwrap();
                    migrate_notebook_db(&live).unwrap();
                    live.execute_batch("PRAGMA wal_autocheckpoint=0; CREATE TABLE wal_retained(value TEXT); PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
                    live.execute_batch("BEGIN IMMEDIATE").unwrap();
                    live.execute(
                        "UPDATE _meta SET value=?1 WHERE key='schema_version'",
                        [version],
                    )
                    .unwrap();
                    live.execute_batch(
                        "INSERT INTO wal_retained VALUES ('committed WAL record'); COMMIT;",
                    )
                    .unwrap();
                    if !condition.contains("stale") {
                        live.execute_batch(
                            "BEGIN IMMEDIATE; INSERT INTO wal_retained VALUES ('uncommitted');",
                        )
                        .unwrap();
                    }
                    writer = Some(live);
                }
                let _keep_copied = copied;
                let case = format!("discriminate-{mode}-{condition}");
                let before = file_state(&path);
                record_file_state(&format!("{case}-before"), &before);
                let result = if mode == "disposable-inspection" {
                    let inspection = tempdir().unwrap();
                    let copy = inspection.path().join("inspection.db");
                    for suffix in ["", "-wal", "-journal"] {
                        let source = sidecar(&path, suffix);
                        if source.exists() {
                            std::fs::copy(source, sidecar(&copy, suffix)).unwrap();
                        }
                    }
                    let conn = Connection::open(copy).unwrap();
                    validate_schema_version(&conn, DatabaseSchema::Notebook)
                } else {
                    let conn = if mode == "no-close-checkpoint" {
                        Connection::open(&path).unwrap()
                    } else {
                        Connection::open_with_flags(
                            format!("file:{}?readonly_shm=1", path.display()),
                            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
                        )
                        .unwrap()
                    };
                    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
                        .unwrap();
                    if mode == "readonly-shm-or-exclusive" && !sidecar(&path, "-shm").exists() {
                        conn.execute_batch("PRAGMA locking_mode=EXCLUSIVE;")
                            .unwrap();
                    }
                    validate_schema_version(&conn, DatabaseSchema::Notebook)
                };
                let after = file_state(&path);
                record_file_state(&format!("{case}-after"), &after);
                let changes: Vec<_> = ["main", "wal", "shm", "journal"]
                    .into_iter()
                    .enumerate()
                    .filter_map(|(i, name)| (before[i] != after[i]).then_some(name))
                    .collect();
                println!(
                    "WAL_CONFIGURATION {}",
                    serde_json::json!({"mode":mode,"condition":condition,"error":format!("{:?}",result.as_ref().err()),"changed_files":changes})
                );
                assert!(result.is_err());
                if mode == "disposable-inspection" {
                    assert_eq!(before, after);
                }
                if let Some(writer) = writer {
                    if !writer.is_autocommit() {
                        writer.execute_batch("ROLLBACK").unwrap();
                    }
                }
            }
        }
    }

    #[test]
    fn public_writer_admissions_accept_supported_committed_wal() {
        use super::wal_admission_fixtures::CopiedWal;
        for (entrypoint, supported_version, open) in schema_guard_entrypoints() {
            for include_shm in [true, false] {
                let directory = tempdir().unwrap();
                let path = directory.path().join("supported.db");
                let schema = if entrypoint == "app_open" {
                    DatabaseSchema::App
                } else {
                    DatabaseSchema::Notebook
                };
                let version = supported_version.to_string();
                let _fixture = CopiedWal::create(&path, schema, &version, include_shm);
                open(&path)
                    .unwrap_or_else(|error| panic!("{entrypoint} rejected supported WAL: {error}"));
                CopiedWal::assert_readable(&path, &version);
            }
        }
    }

    #[test]
    fn supported_wal_connections_remain_usable_with_a_live_notebook_pool() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("live-pool.db");
        let pool = NotebookDbPool::new(&path).unwrap();
        pool.write(|db| {
            db.conn().execute_batch("CREATE TABLE wal_retained(value TEXT NOT NULL); INSERT INTO wal_retained VALUES ('committed WAL record');")?;
            Ok(())
        }).unwrap();
        let connection = NotebookDb::connect(&path).unwrap();
        assert_eq!(
            connection
                .conn()
                .query_row("SELECT value FROM wal_retained", [], |row| row
                    .get::<_, String>(0),)
                .unwrap(),
            "committed WAL record"
        );
        pool.read(|db| {
            assert_eq!(
                db.conn()
                    .query_row("SELECT COUNT(*) FROM wal_retained", [], |row| row
                        .get::<_, i64>(0))?,
                1
            );
            Ok(())
        })
        .unwrap();
        pool.write(|db| {
            db.conn()
                .execute("INSERT INTO wal_retained VALUES ('usable writer')", [])?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            connection
                .conn()
                .query_row("SELECT COUNT(*) FROM wal_retained", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn public_write_entrypoints_reject_invalid_schema_without_persistent_changes() {
        let dir = tempdir().unwrap();
        for (entrypoint, supported_version, open) in schema_guard_entrypoints() {
            let future_version = (supported_version + 1).to_string();
            for (case, schema_value) in
                [future_version.as_str(), "not-a-version", "2147483648", "-1"]
                    .into_iter()
                    .enumerate()
            {
                for (meta_case, meta_table) in ["_meta", "_META"].into_iter().enumerate() {
                    let original = dir
                        .path()
                        .join(format!("{entrypoint}-{case}-{meta_case}.db"));
                    let copied = dir
                        .path()
                        .join(format!("{entrypoint}-{case}-{meta_case}-copy.db"));
                    {
                        let conn = Connection::open(&original).unwrap();
                        conn.execute_batch(&format!(
                            "PRAGMA journal_mode=DELETE;
                             PRAGMA user_version=93;
                             CREATE TABLE {meta_table} (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                             CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                             INSERT INTO settings VALUES ('semantic_memory_embedding_model', 'all-minilm');
                             CREATE TABLE future_owned (id TEXT PRIMARY KEY, value TEXT NOT NULL);
                             INSERT INTO future_owned VALUES ('fixture', 'preserved user data');"
                        ))
                        .unwrap();
                        conn.execute(
                            "INSERT INTO _meta VALUES ('schema_version', ?1)",
                            [schema_value],
                        )
                        .unwrap();
                    }
                    std::fs::copy(&original, &copied).unwrap();
                    let before = std::fs::read(&copied).unwrap();
                    let before_hash = Sha256::digest(&before);

                    let error = open(&copied).unwrap_err();
                    match (case, error) {
                        (
                            0,
                            GlossError::Database(rusqlite::Error::SqliteFailure(_, Some(message))),
                        ) => {
                            assert!(message.contains(&format!("schema version {schema_value}")));
                            assert!(
                                message.contains(&format!("supports up to {supported_version}"))
                            );
                        }
                        (
                            1 | 2,
                            GlossError::Database(rusqlite::Error::FromSqlConversionFailure(
                                _,
                                _,
                                _,
                            )),
                        ) => {}
                        (
                            3,
                            GlossError::Database(rusqlite::Error::SqliteFailure(_, Some(message))),
                        ) => {
                            assert!(message.contains("schema version -1"));
                            assert!(message.contains("expected a non-negative version"));
                        }
                        (_, error) => panic!("{entrypoint} returned unexpected rejection: {error}"),
                    }

                    {
                        let conn = Connection::open_with_flags(
                            &copied,
                            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                        )
                        .unwrap();
                        let retained: String = conn
                            .query_row(
                                "SELECT value FROM _meta WHERE key = 'schema_version'",
                                [],
                                |row| row.get(0),
                            )
                            .unwrap();
                        assert_eq!(retained, schema_value);
                        let journal: String = conn
                            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                            .unwrap();
                        assert_eq!(journal, "delete");
                        let user_version: i32 = conn
                            .query_row("PRAGMA user_version", [], |row| row.get(0))
                            .unwrap();
                        assert_eq!(user_version, 93);
                    }
                    let after = std::fs::read(&copied).unwrap();
                    assert_eq!(
                        after, before,
                        "{entrypoint} modified rejected fixture bytes"
                    );
                    assert_eq!(Sha256::digest(&after), before_hash);
                    assert_eq!(std::fs::read(&original).unwrap(), before);
                    for suffix in ["-wal", "-shm", "-journal"] {
                        let mut sidecar = copied.as_os_str().to_os_string();
                        sidecar.push(suffix);
                        assert!(!Path::new(&sidecar).exists(), "{entrypoint} left {suffix}");
                    }
                }
            }
        }
    }

    #[test]
    fn public_write_entrypoints_accept_current_and_supported_legacy_schema() {
        let dir = tempdir().unwrap();
        for (entrypoint, supported_version, open) in schema_guard_entrypoints() {
            if entrypoint != "notebook_connect" {
                let fresh = dir.path().join(format!("{entrypoint}-fresh.db"));
                open(&fresh).unwrap();
                let conn = Connection::open(&fresh).unwrap();
                assert_eq!(get_schema_version(&conn).unwrap(), supported_version);
            }
            for version in [0, 3, supported_version] {
                let path = dir.path().join(format!("{entrypoint}-{version}.db"));
                if entrypoint == "app_open" {
                    drop(AppDb::open(&path).unwrap());
                } else {
                    drop(NotebookDb::open(&path).unwrap());
                }
                {
                    let conn = Connection::open(&path).unwrap();
                    set_schema_version(&conn, version).unwrap();
                    conn.execute_batch("PRAGMA journal_mode=DELETE").unwrap();
                }

                open(&path).unwrap();
                let conn = Connection::open(&path).unwrap();
                let expected_version = if entrypoint == "notebook_connect" {
                    version
                } else {
                    supported_version
                };
                assert_eq!(get_schema_version(&conn).unwrap(), expected_version);
                let journal: String = conn
                    .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(journal, "wal");
            }
        }
    }

    #[test]
    fn app_fresh_install_seeds_bge_m3_embedding_model() {
        let dir = tempdir().unwrap();
        let db = AppDb::open(&dir.path().join("app.db")).unwrap();
        assert_eq!(
            db.get_setting("semantic_memory_embedding_model")
                .unwrap()
                .as_deref(),
            Some("bge-m3")
        );
        assert_eq!(get_schema_version(db.conn()).unwrap(), APP_SCHEMA_VERSION);
    }

    #[test]
    fn app_reopen_preserves_configured_all_minilm_embedding_model() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("app.db");
        {
            let db = AppDb::open(&path).unwrap();
            db.set_setting("semantic_memory_embedding_model", "all-minilm")
                .unwrap();
        }

        for _ in 0..3 {
            let db = AppDb::open(&path).unwrap();
            assert_eq!(
                db.get_setting("semantic_memory_embedding_model")
                    .unwrap()
                    .as_deref(),
                Some("all-minilm")
            );
            assert_eq!(get_schema_version(db.conn()).unwrap(), APP_SCHEMA_VERSION);
        }
    }

    #[test]
    fn app_legacy_migration_preserves_configured_embedding_models() {
        let dir = tempdir().unwrap();
        for version in 0..APP_SCHEMA_VERSION {
            for (model_index, model) in ["all-minilm", "operator-model:v2"].into_iter().enumerate()
            {
                let path = dir.path().join(format!("app-{version}-{model_index}.db"));
                {
                    let db = AppDb::open(&path).unwrap();
                    db.set_setting("semantic_memory_embedding_model", model)
                        .unwrap();
                    set_schema_version(db.conn(), version).unwrap();
                }

                let db = AppDb::open(&path).unwrap();
                assert_eq!(
                    db.get_setting("semantic_memory_embedding_model")
                        .unwrap()
                        .as_deref(),
                    Some(model),
                    "schema {version} must retain the configured embedding model"
                );
                assert_eq!(get_schema_version(db.conn()).unwrap(), APP_SCHEMA_VERSION);
            }
        }
    }

    #[test]
    fn future_schema_rejection_preserves_copied_database_bytes() {
        let dir = tempdir().unwrap();
        for (database, supported_version) in [
            ("app", APP_SCHEMA_VERSION),
            ("notebook", NOTEBOOK_SCHEMA_VERSION),
        ] {
            let original = dir.path().join(format!("{database}-future-original.db"));
            let copied = dir.path().join(format!("{database}-future-copy.db"));
            let future_version = supported_version + 1;
            {
                let conn = Connection::open(&original).unwrap();
                conn.execute_batch(
                    "PRAGMA journal_mode=DELETE;
                     PRAGMA user_version=93;
                     CREATE TABLE _meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                     CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                     INSERT INTO settings VALUES ('semantic_memory_embedding_model', 'all-minilm');
                     CREATE TABLE future_owned (id TEXT PRIMARY KEY, value TEXT NOT NULL);
                     CREATE INDEX future_owned_value ON future_owned(value);
                     INSERT INTO future_owned VALUES ('fixture', 'future user data');",
                )
                .unwrap();
                set_schema_version(&conn, future_version).unwrap();
            }
            std::fs::copy(&original, &copied).unwrap();
            let before = std::fs::read(&copied).unwrap();

            let error = if database == "app" {
                match AppDb::open(&copied) {
                    Err(GlossError::Database(error)) => error,
                    Err(error) => panic!("expected database rejection, got {error}"),
                    Ok(_) => panic!("future app schema must be rejected"),
                }
            } else {
                let conn = Connection::open(&copied).unwrap();
                migrate_notebook_db(&conn).unwrap_err()
            };
            match error {
                rusqlite::Error::SqliteFailure(error, Some(message)) => {
                    assert_eq!(error.extended_code, rusqlite::ffi::SQLITE_ERROR);
                    assert!(message.contains(&format!(
                        "{database} database schema version {future_version}"
                    )));
                    assert!(message.contains(&format!("supports up to {supported_version}")));
                }
                error => panic!("expected typed future-schema rejection, got {error}"),
            }

            {
                let conn = Connection::open(&copied).unwrap();
                assert_eq!(get_schema_version(&conn).unwrap(), future_version);
                let model: String = conn
                    .query_row(
                        "SELECT value FROM settings WHERE key = 'semantic_memory_embedding_model'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(model, "all-minilm");
                let journal: String = conn
                    .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(journal, "delete");
                let user_version: i32 = conn
                    .query_row("PRAGMA user_version", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(user_version, 93);
            }
            assert_eq!(std::fs::read(&copied).unwrap(), before);
            assert_eq!(std::fs::read(&original).unwrap(), before);
        }
    }

    #[test]
    fn malformed_schema_version_is_rejected_before_migration_writes() {
        for migrate in [migrate_app_db, migrate_notebook_db] {
            for version in ["not-a-version", "2147483648"] {
                let conn = Connection::open_in_memory().unwrap();
                conn.execute_batch(
                    "CREATE TABLE _meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO _meta (key, value) VALUES ('schema_version', ?1)",
                    [version],
                )
                .unwrap();
                assert!(matches!(
                    migrate(&conn),
                    Err(rusqlite::Error::FromSqlConversionFailure(_, _, _))
                ));
                let user_tables: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(user_tables, 1);
                let retained: String = conn
                    .query_row(
                        "SELECT value FROM _meta WHERE key = 'schema_version'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(retained, version);
            }
        }
    }

    #[test]
    fn app_v4_migrates_only_the_exact_legacy_search_timeout_default() {
        let conn = Connection::open_in_memory().unwrap();
        migrate_app_db(&conn).unwrap();
        conn.execute(
            "UPDATE _meta SET value = '3' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE settings SET value = '8000' WHERE key = 'semantic_memory_search_timeout_ms'",
            [],
        )
        .unwrap();

        migrate_app_db(&conn).unwrap();
        let migrated: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'semantic_memory_search_timeout_ms'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(migrated, "60000");
        assert_eq!(get_schema_version(&conn).unwrap(), APP_SCHEMA_VERSION);

        conn.execute(
            "UPDATE _meta SET value = '3' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE settings SET value = '45000' WHERE key = 'semantic_memory_search_timeout_ms'",
            [],
        )
        .unwrap();
        migrate_app_db(&conn).unwrap();
        let retained: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'semantic_memory_search_timeout_ms'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, "45000");
    }

    #[test]
    fn notebook_migration_repairs_missing_fts_for_existing_schema() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE _meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            INSERT INTO _meta (key, value) VALUES ('schema_version', '3');

            CREATE TABLE sources (
                id                TEXT PRIMARY KEY,
                source_type       TEXT NOT NULL,
                title             TEXT NOT NULL,
                original_filename TEXT,
                file_hash         TEXT,
                url               TEXT,
                file_path         TEXT,
                content_text      TEXT,
                word_count        INTEGER,
                metadata          TEXT,
                summary           TEXT,
                summary_model     TEXT,
                status            TEXT DEFAULT 'pending',
                error_message     TEXT,
                selected          INTEGER DEFAULT 1,
                created_at        TEXT DEFAULT (datetime('now')),
                updated_at        TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE chunks (
                id              TEXT PRIMARY KEY,
                source_id       TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
                chunk_index     INTEGER NOT NULL,
                content         TEXT NOT NULL,
                token_count     INTEGER,
                start_offset    INTEGER,
                end_offset      INTEGER,
                metadata        TEXT,
                embedding_id    INTEGER,
                embedding_model TEXT,
                created_at      TEXT DEFAULT (datetime('now'))
            );

            INSERT INTO sources (id, source_type, title, status)
            VALUES ('s1', 'text', 'Source', 'ready');
            INSERT INTO chunks (id, source_id, chunk_index, content)
            VALUES ('c1', 's1', 0, 'repairtoken original');",
        )
        .unwrap();

        migrate_notebook_db(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'repairtoken'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        conn.execute(
            "UPDATE chunks SET content = 'repairtoken updatedtoken' WHERE id = 'c1'",
            [],
        )
        .unwrap();
        let updated_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH 'updatedtoken'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(updated_count, 1);
    }

    #[test]
    fn notebook_migration_repairs_legacy_semantic_memory_links() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE _meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            INSERT INTO _meta (key, value) VALUES ('schema_version', '3');

            CREATE TABLE sources (
                id                TEXT PRIMARY KEY,
                source_type       TEXT NOT NULL,
                title             TEXT NOT NULL,
                original_filename TEXT,
                file_hash         TEXT,
                url               TEXT,
                file_path         TEXT,
                content_text      TEXT,
                word_count        INTEGER,
                metadata          TEXT,
                summary           TEXT,
                summary_model     TEXT,
                status            TEXT DEFAULT 'pending',
                error_message     TEXT,
                selected          INTEGER DEFAULT 1,
                created_at        TEXT DEFAULT (datetime('now')),
                updated_at        TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE chunks (
                id              TEXT PRIMARY KEY,
                source_id       TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
                chunk_index     INTEGER NOT NULL,
                content         TEXT NOT NULL,
                token_count     INTEGER,
                start_offset    INTEGER,
                end_offset      INTEGER,
                metadata        TEXT,
                embedding_id    INTEGER,
                embedding_model TEXT,
                created_at      TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE semantic_memory_links (
                chunk_id TEXT PRIMARY KEY,
                notebook_id TEXT,
                source_id TEXT,
                sm_document_id TEXT,
                content_digest TEXT,
                backend_version TEXT,
                sync_status TEXT,
                sync_error TEXT,
                synced_at TEXT
            );

            INSERT INTO sources (id, source_type, title, status)
            VALUES ('s1', 'text', 'Source', 'ready');
            INSERT INTO chunks (id, source_id, chunk_index, content)
            VALUES ('c1', 's1', 0, 'semantic link repair content');
            INSERT INTO semantic_memory_links (
                chunk_id, notebook_id, source_id, sm_document_id, content_digest,
                backend_version, sync_status, synced_at
            )
            VALUES ('c1', 'nb1', 's1', 'doc1', '', 'legacy', 'synced', '2026-01-01T00:00:00Z');",
        )
        .unwrap();

        migrate_notebook_db(&conn).unwrap();

        assert!(table_has_column(&conn, "semantic_memory_links", "sm_chunk_id").unwrap());
        assert!(table_has_column(&conn, "semantic_memory_links", "sm_episode_id").unwrap());

        let (digest, status, error): (String, String, String) = conn
            .query_row(
                "SELECT content_digest, sync_status, sync_error
                 FROM semantic_memory_links
                 WHERE chunk_id = 'c1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(digest.len(), 64);
        assert_eq!(status, "degraded-missing-exact-backpointer");
        assert_eq!(error, "missing exact semantic chunk identity");
    }

    #[test]
    fn notebook_migration_creates_missing_semantic_memory_links_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE _meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            INSERT INTO _meta (key, value) VALUES ('schema_version', '3');

            CREATE TABLE sources (
                id                TEXT PRIMARY KEY,
                source_type       TEXT NOT NULL,
                title             TEXT NOT NULL,
                original_filename TEXT,
                file_hash         TEXT,
                url               TEXT,
                file_path         TEXT,
                content_text      TEXT,
                word_count        INTEGER,
                metadata          TEXT,
                summary           TEXT,
                summary_model     TEXT,
                status            TEXT DEFAULT 'pending',
                error_message     TEXT,
                selected          INTEGER DEFAULT 1,
                created_at        TEXT DEFAULT (datetime('now')),
                updated_at        TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE chunks (
                id              TEXT PRIMARY KEY,
                source_id       TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
                chunk_index     INTEGER NOT NULL,
                content         TEXT NOT NULL,
                token_count     INTEGER,
                start_offset    INTEGER,
                end_offset      INTEGER,
                metadata        TEXT,
                embedding_id    INTEGER,
                embedding_model TEXT,
                created_at      TEXT DEFAULT (datetime('now'))
            );",
        )
        .unwrap();

        migrate_notebook_db(&conn).unwrap();

        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*)
                 FROM sqlite_master
                 WHERE type = 'table' AND name = 'semantic_memory_links'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 1);
        assert!(table_has_column(&conn, "semantic_memory_links", "sm_chunk_id").unwrap());
    }

    #[test]
    fn notebook_migration_creates_semantic_memory_projection_status_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE _meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            INSERT INTO _meta (key, value) VALUES ('schema_version', '3');

            CREATE TABLE sources (
                id                TEXT PRIMARY KEY,
                source_type       TEXT NOT NULL,
                title             TEXT NOT NULL,
                original_filename TEXT,
                file_hash         TEXT,
                url               TEXT,
                file_path         TEXT,
                content_text      TEXT,
                word_count        INTEGER,
                metadata          TEXT,
                summary           TEXT,
                summary_model     TEXT,
                status            TEXT DEFAULT 'pending',
                error_message     TEXT,
                selected          INTEGER DEFAULT 1,
                created_at        TEXT DEFAULT (datetime('now')),
                updated_at        TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE chunks (
                id              TEXT PRIMARY KEY,
                source_id       TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
                chunk_index     INTEGER NOT NULL,
                content         TEXT NOT NULL,
                token_count     INTEGER,
                start_offset    INTEGER,
                end_offset      INTEGER,
                metadata        TEXT,
                embedding_id    INTEGER,
                embedding_model TEXT,
                created_at      TEXT DEFAULT (datetime('now'))
            );",
        )
        .unwrap();

        migrate_notebook_db(&conn).unwrap();

        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*)
                 FROM sqlite_master
                 WHERE type = 'table' AND name = 'semantic_memory_projection_status'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 1);
        assert!(table_has_column(&conn, "semantic_memory_projection_status", "status").unwrap());
        assert!(table_has_column(
            &conn,
            "semantic_memory_projection_status",
            "vector_artifact_manifest_digest"
        )
        .unwrap());

        for table in [
            "semantic_memory_vector_artifact_receipts",
            "semantic_memory_retrieval_probe_receipts",
        ] {
            let table_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*)
                     FROM sqlite_master
                     WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(table_count, 1);
        }
    }
}
