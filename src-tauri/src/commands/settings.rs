use crate::db::app_db::{ModelRecord, Provider};
use crate::error::GlossError;
use crate::providers::ollama::OllamaProvider;
use crate::providers::{LlmProvider, ModelInfo, ModelRegistry};
use crate::state::AppState;
use std::collections::HashMap;
use tauri::State;

#[tauri::command]
pub async fn get_providers(state: State<'_, AppState>) -> Result<Vec<Provider>, GlossError> {
    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    app_db.list_providers()
}

#[tauri::command]
pub async fn update_provider(
    id: String,
    enabled: bool,
    base_url: Option<String>,
    api_key: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), GlossError> {
    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    app_db.update_provider(&id, enabled, base_url.as_deref(), api_key.as_deref())
}

#[tauri::command]
pub async fn test_provider(
    provider_id: String,
    state: State<'_, AppState>,
) -> Result<bool, GlossError> {
    // Get the base URL without holding the lock across await
    let base_url = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        app_db
            .get_setting("ollama_url")?
            .unwrap_or_else(|| "http://localhost:11434".to_string())
    };

    match provider_id.as_str() {
        "ollama" => {
            let provider = OllamaProvider::new(&base_url);
            provider.health_check().await
        }
        _ => Ok(false),
    }
}

#[tauri::command]
pub async fn refresh_models(
    _provider_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<ModelInfo>, GlossError> {
    // Get the Ollama URL without holding lock across await
    let base_url = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        app_db
            .get_setting("ollama_url")?
            .unwrap_or_else(|| "http://localhost:11434".to_string())
    };

    // Fetch models directly (no lock held)
    let provider = OllamaProvider::new(&base_url);
    let models = provider.list_models().await?;

    // Store in DB and update registry (lock held briefly, no await)
    {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let records = ModelRegistry::to_model_records(&models);
        let ollama_records: Vec<ModelRecord> = records
            .into_iter()
            .filter(|r| r.provider_id == "ollama")
            .collect();
        app_db.replace_models("ollama", &ollama_records)?;
    }

    {
        let mut registry = state
            .model_registry
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        registry.cached_models = models.clone();
    }

    Ok(models)
}

#[tauri::command]
pub async fn get_all_models(state: State<'_, AppState>) -> Result<Vec<ModelRecord>, GlossError> {
    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    app_db.get_all_models()
}

#[tauri::command]
pub async fn get_settings(
    state: State<'_, AppState>,
) -> Result<HashMap<String, String>, GlossError> {
    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    app_db.get_settings()
}

#[tauri::command]
pub async fn update_setting(
    key: String,
    value: String,
    state: State<'_, AppState>,
) -> Result<(), GlossError> {
    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    app_db.set_setting(&key, &value)
}
