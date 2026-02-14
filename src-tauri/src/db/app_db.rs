use crate::db::migrations;
use crate::error::GlossError;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// App-level database handle (gloss.db).
pub struct AppDb {
    pub conn: Connection,
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
    pub fn create_notebook(
        &self,
        id: &str,
        name: &str,
        directory: &str,
    ) -> Result<(), GlossError> {
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

    /// Update last_accessed timestamp for a notebook.
    pub fn touch_notebook(&self, id: &str) -> Result<(), GlossError> {
        self.conn.execute(
            "UPDATE notebooks SET last_accessed = datetime('now'), updated_at = datetime('now') WHERE id = ?1",
            [id],
        )?;
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
        let mut stmt = self
            .conn
            .prepare("SELECT id, enabled, base_url, api_key, last_refreshed FROM providers")?;
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
    pub fn update_provider(
        &self,
        id: &str,
        enabled: bool,
        base_url: Option<&str>,
        api_key: Option<&str>,
    ) -> Result<(), GlossError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO providers (id, enabled, base_url, api_key)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, enabled, base_url, api_key],
        )?;
        Ok(())
    }

    /// Get a provider's base URL.
    pub fn get_provider_url(&self, id: &str) -> Result<Option<String>, GlossError> {
        let url = self.conn.query_row(
            "SELECT base_url FROM providers WHERE id = ?1",
            [id],
            |row| row.get(0),
        ).map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                GlossError::NotFound(format!("Provider {id} not found"))
            }
            other => GlossError::Database(other),
        })?;
        Ok(url)
    }

    // -- Models --

    /// Replace cached models for a provider.
    pub fn replace_models(
        &self,
        provider_id: &str,
        models: &[ModelRecord],
    ) -> Result<(), GlossError> {
        self.conn.execute(
            "DELETE FROM models WHERE provider_id = ?1",
            [provider_id],
        )?;
        let mut stmt = self.conn.prepare(
            "INSERT INTO models (id, provider_id, display_name, parameter_size, context_window, capabilities)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for m in models {
            stmt.execute(rusqlite::params![
                m.id,
                m.provider_id,
                m.display_name,
                m.parameter_size,
                m.context_window,
                m.capabilities,
            ])?;
        }
        self.conn.execute(
            "UPDATE providers SET last_refreshed = datetime('now') WHERE id = ?1",
            [provider_id],
        )?;
        Ok(())
    }

    /// Get all cached models.
    pub fn get_all_models(&self) -> Result<Vec<ModelRecord>, GlossError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, provider_id, display_name, parameter_size, context_window, capabilities
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
            })
        })?;
        let mut models = Vec::new();
        for row in rows {
            models.push(row?);
        }
        Ok(models)
    }

    // -- Settings --

    /// Get all settings as key-value pairs.
    pub fn get_settings(&self) -> Result<std::collections::HashMap<String, String>, GlossError> {
        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM settings")?;
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
        let result = self.conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [key],
            |row| row.get(0),
        );
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
    fn test_settings() {
        let db = test_db();
        // Default settings should exist
        let settings = db.get_settings().unwrap();
        assert_eq!(settings.get("default_provider").unwrap(), "ollama");

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
    fn test_models_crud() {
        let db = test_db();
        let models = vec![ModelRecord {
            id: "qwen3:8b".to_string(),
            provider_id: "ollama".to_string(),
            display_name: "Qwen3 8B".to_string(),
            parameter_size: Some("8.2B".to_string()),
            context_window: Some(32768),
            capabilities: None,
        }];
        db.replace_models("ollama", &models).unwrap();
        let all = db.get_all_models().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].display_name, "Qwen3 8B");
    }
}
