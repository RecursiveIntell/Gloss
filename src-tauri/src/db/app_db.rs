use crate::db::migrations;
use crate::error::GlossError;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// App-level database handle (gloss.db).
pub struct AppDb {
    conn: Connection,
}

impl AppDb {
    /// Provide temporary read-only access to the underlying connection.
    ///
    /// This is the only sanctioned way for callers outside `db::app_db`
    /// to reach the `rusqlite::Connection`.  The field itself is private
    /// to prevent accidental direct mutation or schema changes.
    #[allow(dead_code)]
    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notebook {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub directory: String,
    pub source_count: i32,
    pub last_accessed: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub enabled: bool,
    pub base_url: Option<String>,
    pub has_api_key: bool,
    pub last_refreshed: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRecord {
    pub id: String,
    pub provider_id: String,
    pub display_name: String,
    pub parameter_size: Option<String>,
    pub context_window: Option<i32>,
    pub capabilities: Option<String>,
    pub available: bool,
    pub stale: bool,
    pub last_error: Option<String>,
}

impl AppDb {
    /// Open (or create) the app-level database.
    pub fn open(path: &Path) -> Result<Self, GlossError> {
        let conn = Connection::open(path)?;
        migrations::migrate_app_db(&conn)?;
        Ok(Self { conn })
    }

    // -- Notebooks --

    /// List all notebooks ordered by last accessed.
    pub fn list_notebooks(&self) -> Result<Vec<Notebook>, GlossError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, directory, source_count, last_accessed, created_at, updated_at
             FROM notebooks ORDER BY last_accessed DESC NULLS LAST, created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Notebook {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                directory: row.get(3)?,
                source_count: row.get(4)?,
                last_accessed: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })?;
        let mut notebooks = Vec::new();
        for row in rows {
            notebooks.push(row?);
        }
        Ok(notebooks)
    }

    /// Create a new notebook. Returns the ID.
    pub fn create_notebook(&self, id: &str, name: &str, directory: &str) -> Result<(), GlossError> {
        self.conn.execute(
            "INSERT INTO notebooks (id, name, directory) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, directory],
        )?;
        Ok(())
    }

    /// Get a notebook by ID.
    pub fn get_notebook(&self, id: &str) -> Result<Notebook, GlossError> {
        self.conn
            .query_row(
                "SELECT id, name, description, directory, source_count, last_accessed, created_at, updated_at
                 FROM notebooks WHERE id = ?1",
                [id],
                |row| {
                    Ok(Notebook {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        description: row.get(2)?,
                        directory: row.get(3)?,
                        source_count: row.get(4)?,
                        last_accessed: row.get(5)?,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    GlossError::NotFound(format!("Notebook {id} not found"))
                }
                other => GlossError::Database(other),
            })
    }

    /// Delete a notebook from the registry.
    pub fn delete_notebook(&self, id: &str) -> Result<(), GlossError> {
        self.conn
            .execute("DELETE FROM notebooks WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Rename a notebook.
    pub fn rename_notebook(&self, id: &str, name: &str) -> Result<(), GlossError> {
        self.conn.execute(
            "UPDATE notebooks SET name = ?1, updated_at = datetime('now') WHERE id = ?2",
            rusqlite::params![name, id],
        )?;
        Ok(())
    }

    /// Update last_accessed timestamp for a notebook.
    pub fn touch_notebook(&self, id: &str) -> Result<(), GlossError> {
        let changed = self.conn.execute(
            "UPDATE notebooks SET last_accessed = datetime('now'), updated_at = datetime('now') WHERE id = ?1",
            [id],
        )?;
        if changed != 1 {
            return Err(GlossError::NotFound(format!("Notebook {id} not found")));
        }
        Ok(())
    }

    /// Update source_count for a notebook.
    pub fn update_source_count(&self, id: &str, count: i32) -> Result<(), GlossError> {
        self.conn.execute(
            "UPDATE notebooks SET source_count = ?1, updated_at = datetime('now') WHERE id = ?2",
            rusqlite::params![count, id],
        )?;
        Ok(())
    }

    // -- Providers --

    /// List all providers.
    pub fn list_providers(&self) -> Result<Vec<Provider>, GlossError> {
        let query = if self.table_has_column("providers", "api_key")? {
            "SELECT id, enabled, base_url, api_key, last_refreshed FROM providers"
        } else {
            "SELECT id, enabled, base_url, NULL AS api_key, last_refreshed FROM providers"
        };
        let mut stmt = self.conn.prepare(query)?;
        let rows = stmt.query_map([], |row| {
            let api_key: Option<String> = row.get(3)?;
            Ok(Provider {
                id: row.get(0)?,
                enabled: row.get(1)?,
                base_url: row.get(2)?,
                has_api_key: api_key.is_some(),
                last_refreshed: row.get(4)?,
            })
        })?;
        let mut providers = Vec::new();
        for row in rows {
            providers.push(row?);
        }
        Ok(providers)
    }

    /// Update a provider configuration.
    /// API keys are not stored in the DB — they go through SecretStore.
    pub fn update_provider(
        &self,
        id: &str,
        enabled: bool,
        base_url: Option<&str>,
    ) -> Result<(), GlossError> {
        // Preserve the existing base_url when the caller passes None. If no
        // row exists yet (first time we enable a provider), fall back to None
        // so the INSERT OR REPLACE below still creates a row.
        let preserved_base_url = match base_url {
            Some(value) => Some(value.to_string()),
            None => match self.get_provider_url(id) {
                Ok(existing) => existing,
                Err(GlossError::NotFound(_)) => None,
                Err(other) => return Err(other),
            },
        };
        self.conn.execute(
            "INSERT OR REPLACE INTO providers (id, enabled, base_url)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![id, enabled, preserved_base_url.as_deref()],
        )?;
        Ok(())
    }

    pub fn get_provider_api_key(&self, id: &str) -> Result<Option<String>, GlossError> {
        if !self.table_has_column("providers", "api_key")? {
            return Ok(None);
        }
        let result =
            self.conn
                .query_row("SELECT api_key FROM providers WHERE id = ?1", [id], |row| {
                    row.get(0)
                });

        match result {
            Ok(value) => Ok(value),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(GlossError::Database(e)),
        }
    }

    pub fn clear_provider_api_key(&self, id: &str) -> Result<(), GlossError> {
        if !self.table_has_column("providers", "api_key")? {
            return Ok(());
        }
        self.conn
            .execute("UPDATE providers SET api_key = NULL WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Get a provider's base URL.
    #[allow(dead_code)]
    pub fn get_provider_url(&self, id: &str) -> Result<Option<String>, GlossError> {
        let url = self
            .conn
            .query_row(
                "SELECT base_url FROM providers WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    GlossError::NotFound(format!("Provider {id} not found"))
                }
                other => GlossError::Database(other),
            })?;
        Ok(url)
    }

    // -- Models --

    /// Replace cached models for a provider.
    /// Wrapped in a transaction so a crash between DELETE and INSERT cannot
    /// wipe the model list.
    pub fn replace_models(
        &self,
        provider_id: &str,
        models: &[ModelRecord],
    ) -> Result<(), GlossError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM models WHERE provider_id = ?1", [provider_id])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO models (id, provider_id, display_name, parameter_size, context_window, capabilities, available, stale, last_error)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for m in models {
                stmt.execute(rusqlite::params![
                    m.id,
                    m.provider_id,
                    m.display_name,
                    m.parameter_size,
                    m.context_window,
                    m.capabilities,
                    m.available,
                    m.stale,
                    m.last_error,
                ])?;
            }
        }
        tx.execute(
            "UPDATE providers SET last_refreshed = datetime('now') WHERE id = ?1",
            [provider_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Mark a provider's cached models unavailable after a failed refresh.
    pub fn mark_models_unavailable(
        &self,
        provider_id: &str,
        error: &str,
    ) -> Result<(), GlossError> {
        self.conn.execute(
            "UPDATE models
             SET available = 0,
                 stale = 1,
                 last_error = ?2
             WHERE provider_id = ?1",
            rusqlite::params![provider_id, error],
        )?;
        Ok(())
    }

    /// Get all cached models.
    pub fn get_all_models(&self) -> Result<Vec<ModelRecord>, GlossError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, provider_id, display_name, parameter_size, context_window, capabilities, available, stale, last_error
             FROM models ORDER BY provider_id, display_name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ModelRecord {
                id: row.get(0)?,
                provider_id: row.get(1)?,
                display_name: row.get(2)?,
                parameter_size: row.get(3)?,
                context_window: row.get(4)?,
                capabilities: row.get(5)?,
                available: row.get(6)?,
                stale: row.get(7)?,
                last_error: row.get(8)?,
            })
        })?;
        let mut models = Vec::new();
        for row in rows {
            models.push(row?);
        }
        Ok(models)
    }

    // -- Settings --

    /// Validate and persist the provider/model identity as one atomic setting.
    /// Callers hold AppState's app_db lock across validation and commit.
    pub fn select_chat_model(&self, provider_id: &str, model_id: &str) -> Result<(), GlossError> {
        let enabled = self
            .list_providers()?
            .iter()
            .any(|p| p.id == provider_id && p.enabled);
        let ready = self
            .get_all_models()?
            .iter()
            .any(|m| m.provider_id == provider_id && m.id == model_id && m.available && !m.stale);
        if !enabled || !ready {
            return Err(GlossError::Config(format!(
                "Model '{model_id}' is unavailable for enabled provider '{provider_id}'"
            )));
        }
        self.set_settings_atomically(&[
            ("default_provider", provider_id),
            ("default_model", model_id),
        ])
    }

    /// Get all settings as key-value pairs.
    pub fn get_settings(&self) -> Result<std::collections::HashMap<String, String>, GlossError> {
        let mut stmt = self.conn.prepare("SELECT key, value FROM settings")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut settings = std::collections::HashMap::new();
        for row in rows {
            let (k, v) = row?;
            settings.insert(k, v);
        }
        Ok(settings)
    }

    /// Get a single setting.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>, GlossError> {
        let result =
            self.conn
                .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                    row.get(0)
                });
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(GlossError::Database(e)),
        }
    }

    /// Set a setting value.
    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), GlossError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    /// Apply a settings profile as one SQLite unit so runtime gate settings do
    /// not briefly disagree with backend selection.
    pub fn set_settings_atomically(&self, settings: &[(&str, &str)]) -> Result<(), GlossError> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        for (key, value) in settings {
            if let Err(error) = self.conn.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                rusqlite::params![key, value],
            ) {
                let _ = self.conn.execute_batch("ROLLBACK");
                return Err(GlossError::Database(error));
            }
        }
        if let Err(error) = self.conn.execute_batch("COMMIT") {
            let _ = self.conn.execute_batch("ROLLBACK");
            return Err(GlossError::Database(error));
        }
        Ok(())
    }

    fn table_has_column(&self, table: &str, column: &str) -> Result<bool, GlossError> {
        let mut stmt = self.conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        for row in rows {
            if row? == column {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod audit_model_selection_tests {
    use super::*;

    fn fixture() -> AppDb {
        let db = AppDb::open(Path::new(":memory:")).unwrap();
        db.update_provider("openai", true, None).unwrap();
        db.replace_models(
            "openai",
            &[ModelRecord {
                id: "new".into(),
                provider_id: "openai".into(),
                display_name: "New".into(),
                parameter_size: None,
                context_window: None,
                capabilities: None,
                available: true,
                stale: false,
                last_error: None,
            }],
        )
        .unwrap();
        db.set_settings_atomically(&[("default_provider", "ollama"), ("default_model", "old")])
            .unwrap();
        db
    }

    #[test]
    fn exact_enabled_provider_and_model_commit_together() {
        let db = fixture();
        db.select_chat_model("openai", "new").unwrap();
        assert_eq!(
            db.get_setting("default_provider").unwrap().as_deref(),
            Some("openai")
        );
        assert_eq!(
            db.get_setting("default_model").unwrap().as_deref(),
            Some("new")
        );
    }

    #[test]
    fn rejected_selection_does_not_change_either_setting() {
        let db = fixture();
        assert!(db.select_chat_model("ollama", "new").is_err());
        db.update_provider("openai", false, None).unwrap();
        assert!(db.select_chat_model("openai", "new").is_err());
        db.update_provider("openai", true, None).unwrap();
        db.conn
            .execute(
                "UPDATE models SET stale = 1 WHERE provider_id = 'openai'",
                [],
            )
            .unwrap();
        assert!(db.select_chat_model("openai", "new").is_err());
        assert_eq!(
            db.get_setting("default_provider").unwrap().as_deref(),
            Some("ollama")
        );
        assert_eq!(
            db.get_setting("default_model").unwrap().as_deref(),
            Some("old")
        );
    }

    #[test]
    fn second_write_failure_rolls_back_the_provider() {
        let db = fixture();
        db.conn
            .execute_batch(
                "CREATE TRIGGER reject_new_model BEFORE INSERT ON settings
            WHEN NEW.key = 'default_model' AND NEW.value = 'new'
            BEGIN SELECT RAISE(ABORT, 'simulated write failure'); END;",
            )
            .unwrap();
        assert!(db.select_chat_model("openai", "new").is_err());
        assert_eq!(
            db.get_setting("default_provider").unwrap().as_deref(),
            Some("ollama")
        );
        assert_eq!(
            db.get_setting("default_model").unwrap().as_deref(),
            Some("old")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_db() -> AppDb {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test_app.db");
        AppDb::open(&path).unwrap()
    }

    #[test]
    fn test_create_and_list_notebooks() {
        let db = test_db();
        db.create_notebook("nb1", "Test Notebook", "/tmp/nb1")
            .unwrap();
        let notebooks = db.list_notebooks().unwrap();
        assert_eq!(notebooks.len(), 1);
        assert_eq!(notebooks[0].name, "Test Notebook");
    }

    #[test]
    fn test_delete_notebook() {
        let db = test_db();
        db.create_notebook("nb1", "Test", "/tmp/nb1").unwrap();
        db.delete_notebook("nb1").unwrap();
        let notebooks = db.list_notebooks().unwrap();
        assert_eq!(notebooks.len(), 0);
    }

    #[test]
    fn test_rename_notebook() {
        let db = test_db();
        db.create_notebook("nb1", "Before", "/tmp/nb1").unwrap();
        db.rename_notebook("nb1", "After").unwrap();
        let notebook = db.get_notebook("nb1").unwrap();
        assert_eq!(notebook.name, "After");
    }

    #[test]
    fn activation_touch_rejects_missing_notebook_and_write_failure() {
        let db = test_db();
        assert!(matches!(
            db.touch_notebook("missing"),
            Err(GlossError::NotFound(_))
        ));
        db.create_notebook("nb1", "Present", "/tmp/nb1").unwrap();
        db.touch_notebook("nb1").unwrap();
        db.conn
            .execute_batch(
                "CREATE TRIGGER reject_touch BEFORE UPDATE OF last_accessed ON notebooks
             BEGIN SELECT RAISE(FAIL, 'fixture write failure'); END;",
            )
            .unwrap();
        assert!(matches!(
            db.touch_notebook("nb1"),
            Err(GlossError::Database(_))
        ));
        assert_eq!(db.get_notebook("nb1").unwrap().name, "Present");
    }

    #[test]
    fn test_update_source_count() {
        let db = test_db();
        db.create_notebook("nb1", "Counted", "/tmp/nb1").unwrap();
        db.update_source_count("nb1", 7).unwrap();
        let notebook = db.get_notebook("nb1").unwrap();
        assert_eq!(notebook.source_count, 7);
    }

    #[test]
    fn test_settings() {
        let db = test_db();
        // Default settings should exist
        let settings = db.get_settings().unwrap();
        assert_eq!(settings.get("default_provider").unwrap(), "ollama");
        assert_eq!(settings.get("summary_mode").unwrap(), "manual");

        // Set and get
        db.set_setting("theme", "dark").unwrap();
        let val = db.get_setting("theme").unwrap();
        assert_eq!(val, Some("dark".to_string()));
    }

    #[test]
    fn test_providers() {
        let db = test_db();
        let providers = db.list_providers().unwrap();
        assert!(!providers.is_empty());
        assert_eq!(providers[0].id, "ollama");
        assert!(providers[0].enabled);
    }

    #[test]
    fn test_update_provider_does_not_persist_api_key() {
        let db = test_db();
        db.update_provider("openai", true, Some("https://api.openai.com/v1"))
            .unwrap();
        // API keys are handled exclusively via SecretStore, never the providers table
        assert_eq!(db.get_provider_api_key("openai").unwrap(), None);
    }

    #[test]
    fn test_update_provider_preserves_existing_base_url_when_omitted() {
        let db = test_db();
        db.update_provider("openai", true, Some("https://custom.openai.local/v1"))
            .unwrap();

        db.update_provider("openai", true, None).unwrap();

        assert_eq!(
            db.get_provider_url("openai").unwrap(),
            Some("https://custom.openai.local/v1".to_string())
        );
    }

    #[test]
    fn test_update_provider_inserts_new_row_when_missing_and_base_url_none() {
        // Migrations seed rows for ollama/openai/anthropic/llamacpp, so a
        // genuinely missing row needs an unseeded provider id.
        let db = test_db();
        // Row does not exist yet — base_url=None must NOT propagate NotFound,
        // and the row should be inserted with base_url=NULL.
        db.update_provider("customprov", true, None).unwrap();

        assert_eq!(db.get_provider_url("customprov").unwrap(), None);
        let providers = db.list_providers().unwrap();
        let custom = providers
            .iter()
            .find(|p| p.id == "customprov")
            .expect("customprov row");
        assert!(custom.enabled);
    }

    #[test]
    fn test_models_crud() {
        let db = test_db();
        let models = vec![ModelRecord {
            id: "qwen3:8b".to_string(),
            provider_id: "ollama".to_string(),
            display_name: "Qwen3 8B".to_string(),
            parameter_size: Some("8.2B".to_string()),
            context_window: Some(32768),
            capabilities: None,
            available: true,
            stale: false,
            last_error: None,
        }];
        db.replace_models("ollama", &models).unwrap();
        let all = db.get_all_models().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].display_name, "Qwen3 8B");
    }

    /// Verifies that replace_models is atomic: old models are fully replaced
    /// by new models with no intermediate state visible.
    #[test]
    fn test_replace_models_atomic() {
        let db = test_db();

        // Insert initial models
        let old_models = vec![
            ModelRecord {
                id: "model-a".to_string(),
                provider_id: "ollama".to_string(),
                display_name: "Model A".to_string(),
                parameter_size: None,
                context_window: None,
                capabilities: None,
                available: true,
                stale: false,
                last_error: None,
            },
            ModelRecord {
                id: "model-b".to_string(),
                provider_id: "ollama".to_string(),
                display_name: "Model B".to_string(),
                parameter_size: None,
                context_window: None,
                capabilities: None,
                available: true,
                stale: false,
                last_error: None,
            },
        ];
        db.replace_models("ollama", &old_models).unwrap();

        // Verify old models are present
        let all = db.get_all_models().unwrap();
        assert_eq!(all.len(), 2);

        // Replace with a completely new set
        let new_models = vec![
            ModelRecord {
                id: "model-x".to_string(),
                provider_id: "ollama".to_string(),
                display_name: "Model X".to_string(),
                parameter_size: Some("7B".to_string()),
                context_window: Some(8192),
                capabilities: None,
                available: true,
                stale: false,
                last_error: None,
            },
            ModelRecord {
                id: "model-y".to_string(),
                provider_id: "ollama".to_string(),
                display_name: "Model Y".to_string(),
                parameter_size: Some("13B".to_string()),
                context_window: Some(16384),
                capabilities: None,
                available: true,
                stale: false,
                last_error: None,
            },
            ModelRecord {
                id: "model-z".to_string(),
                provider_id: "ollama".to_string(),
                display_name: "Model Z".to_string(),
                parameter_size: None,
                context_window: None,
                capabilities: None,
                available: true,
                stale: false,
                last_error: None,
            },
        ];
        db.replace_models("ollama", &new_models).unwrap();

        // After successful replace, old models must be gone and only new ones present
        let all = db.get_all_models().unwrap();
        assert_eq!(all.len(), 3);
        let names: Vec<&str> = all.iter().map(|m| m.display_name.as_str()).collect();
        assert!(names.contains(&"Model X"));
        assert!(names.contains(&"Model Y"));
        assert!(names.contains(&"Model Z"));
        assert!(!names.contains(&"Model A"));
        assert!(!names.contains(&"Model B"));
    }

    /// Verifies that replace_models rolls back when a constraint is violated
    /// (e.g. duplicate primary key within the same batch) so old models remain.
    #[test]
    fn test_replace_models_rollback_on_constraint_violation() {
        let db = test_db();

        // Insert initial models
        let old_models = vec![ModelRecord {
            id: "original".to_string(),
            provider_id: "ollama".to_string(),
            display_name: "Original Model".to_string(),
            parameter_size: None,
            context_window: None,
            capabilities: None,
            available: true,
            stale: false,
            last_error: None,
        }];
        db.replace_models("ollama", &old_models).unwrap();

        // Try to replace with models that have a duplicate PRIMARY KEY (id, provider_id)
        // within the same batch — this will cause a constraint violation on the second insert.
        let bad_models = vec![
            ModelRecord {
                id: "dup".to_string(),
                provider_id: "ollama".to_string(),
                display_name: "Dup First".to_string(),
                parameter_size: None,
                context_window: None,
                capabilities: None,
                available: true,
                stale: false,
                last_error: None,
            },
            ModelRecord {
                id: "dup".to_string(), // same (id, provider_id) — violates PRIMARY KEY
                provider_id: "ollama".to_string(),
                display_name: "Dup Second".to_string(),
                parameter_size: None,
                context_window: None,
                capabilities: None,
                available: true,
                stale: false,
                last_error: None,
            },
        ];
        let result = db.replace_models("ollama", &bad_models);
        assert!(
            result.is_err(),
            "replace_models should fail on duplicate PK"
        );

        // Old models must still be present after rollback
        let all = db.get_all_models().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].display_name, "Original Model");
    }
}
