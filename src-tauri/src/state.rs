use crate::db::app_db::AppDb;
use crate::db::notebook_db::NotebookDb;
use crate::error::GlossError;
use crate::providers::ModelRegistry;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::AppHandle;

/// Global application state managed by Tauri
pub struct AppState {
    /// App-level database (gloss.db)
    pub app_db: Mutex<AppDb>,
    /// Open notebook databases keyed by notebook ID
    pub notebook_dbs: Mutex<HashMap<String, NotebookDb>>,
    /// LLM provider registry
    pub model_registry: Mutex<ModelRegistry>,
    /// Application data directory
    pub data_dir: PathBuf,
}

impl AppState {
    /// Initialize application state on startup.
    pub fn initialize(_app_handle: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        let data_dir = directories::ProjectDirs::from("com", "sikmindz", "Gloss")
            .ok_or_else(|| GlossError::Config("Could not determine data directory".into()))?
            .data_dir()
            .to_path_buf();

        std::fs::create_dir_all(&data_dir)?;
        std::fs::create_dir_all(data_dir.join("notebooks"))?;

        let db_path = data_dir.join("gloss.db");
        let app_db = AppDb::open(&db_path)?;

        let model_registry = ModelRegistry::new(&app_db)?;

        tracing::info!(data_dir = %data_dir.display(), "Gloss initialized");

        Ok(Self {
            app_db: Mutex::new(app_db),
            notebook_dbs: Mutex::new(HashMap::new()),
            model_registry: Mutex::new(model_registry),
            data_dir,
        })
    }

    /// Get or open a notebook database.
    pub fn get_notebook_db(&self, notebook_id: &str) -> Result<(), GlossError> {
        let mut dbs = self.notebook_dbs.lock().map_err(|e| GlossError::Other(e.to_string()))?;
        if !dbs.contains_key(notebook_id) {
            let app_db = self.app_db.lock().map_err(|e| GlossError::Other(e.to_string()))?;
            let notebook = app_db.get_notebook(notebook_id)?;
            let nb_dir = PathBuf::from(&notebook.directory);
            let db_path = nb_dir.join("notebook.db");
            let nb_db = NotebookDb::open(&db_path)?;
            dbs.insert(notebook_id.to_string(), nb_db);
        }
        Ok(())
    }

    /// Execute a function with a locked notebook database.
    pub fn with_notebook_db<F, T>(&self, notebook_id: &str, f: F) -> Result<T, GlossError>
    where
        F: FnOnce(&NotebookDb) -> Result<T, GlossError>,
    {
        self.get_notebook_db(notebook_id)?;
        let dbs = self.notebook_dbs.lock().map_err(|e| GlossError::Other(e.to_string()))?;
        let db = dbs.get(notebook_id).ok_or_else(|| {
            GlossError::NotFound(format!("Notebook {} not opened", notebook_id))
        })?;
        f(db)
    }
}
