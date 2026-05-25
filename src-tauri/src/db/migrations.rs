use rusqlite::Connection;
use sha2::{Digest, Sha256};

const APP_SCHEMA_VERSION: i32 = 2;
const NOTEBOOK_SCHEMA_VERSION: i32 = 5;

/// Apply pragmas for performance and correctness.
pub fn apply_pragmas(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA foreign_keys=ON;
         PRAGMA busy_timeout=5000;",
    )
}

/// Create or migrate the app-level database schema.
pub fn migrate_app_db(conn: &Connection) -> rusqlite::Result<()> {
    apply_pragmas(conn)?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )?;

    let version = get_schema_version(conn);

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
             INSERT OR IGNORE INTO settings (key, value) VALUES ('default_model', 'qwen3:8b');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('default_embedding_model', 'NomicEmbedTextV15');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('summary_mode', 'manual');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('memory_backend', 'gloss-local');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('memory_backend_fallback', 'true');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('semantic_memory_auto_project', 'false');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('semantic_memory_strict_testing', 'false');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('semantic_memory_turbo_quant_require_fresh_artifacts', 'true');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('semantic_memory_embedding_url', 'http://localhost:11434');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('semantic_memory_embedding_model', 'nomic-embed-text');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('semantic_memory_embedding_timeout_secs', '10');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('semantic_memory_search_timeout_ms', '8000');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('ollama_url', 'http://localhost:11434');
             INSERT OR IGNORE INTO settings (key, value) VALUES ('theme', 'system');",
        )?;

        // Insert default Ollama provider
        conn.execute_batch(
            "INSERT OR IGNORE INTO providers (id, enabled, base_url)
             VALUES ('ollama', 1, 'http://localhost:11434');",
        )?;

        set_schema_version(conn, APP_SCHEMA_VERSION)?;
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
        set_schema_version(conn, APP_SCHEMA_VERSION)?;
    }

    conn.execute_batch(
        "INSERT OR IGNORE INTO settings (key, value) VALUES ('semantic_memory_strict_testing', 'false');
         INSERT OR IGNORE INTO settings (key, value) VALUES ('semantic_memory_turbo_quant_require_fresh_artifacts', 'true');",
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
    apply_pragmas(conn)?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )?;

    let version = get_schema_version(conn);

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

    ensure_notebook_fts(conn)?;
    ensure_semantic_memory_links(conn)?;
    ensure_semantic_memory_projection_status(conn)?;
    ensure_semantic_memory_vector_artifact_receipts(conn)?;
    ensure_semantic_memory_retrieval_probe_receipts(conn)?;
    ensure_provenance_receipts(conn)?;

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
         SET sync_status = 'degraded-missing-exact-backpointer',
             sync_error = COALESCE(sync_error, 'missing exact semantic chunk identity'),
             synced_at = COALESCE(synced_at, datetime('now'))
         WHERE sync_status = 'synced'
           AND (sm_chunk_id IS NULL OR trim(sm_chunk_id) = '')",
        [],
    )?;

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

fn get_schema_version(conn: &Connection) -> i32 {
    conn.query_row(
        "SELECT value FROM _meta WHERE key = 'schema_version'",
        [],
        |row| {
            let v: String = row.get(0)?;
            Ok(v.parse::<i32>().unwrap_or(0))
        },
    )
    .unwrap_or(0)
}

fn set_schema_version(conn: &Connection, version: i32) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO _meta (key, value) VALUES ('schema_version', ?1)",
        [version.to_string()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
