use rusqlite::Connection;

const APP_SCHEMA_VERSION: i32 = 1;
const NOTEBOOK_SCHEMA_VERSION: i32 = 1;

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
                api_key        TEXT,
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

    Ok(())
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

        set_schema_version(conn, NOTEBOOK_SCHEMA_VERSION)?;
    }

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
