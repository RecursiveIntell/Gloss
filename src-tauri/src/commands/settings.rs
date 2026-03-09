use crate::db::app_db::{ModelRecord, Provider};
use crate::error::GlossError;
use crate::providers::{self, ModelInfo, ModelRegistry, ProviderType};
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
    {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        app_db.update_provider(&id, enabled, base_url.as_deref(), api_key.as_deref())?;
    }

    // Rebuild the model registry to pick up new provider config
    {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let new_registry = ModelRegistry::new(&app_db)?;
        let mut registry = state
            .model_registry
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        // Preserve cached models across rebuild
        let cached = std::mem::take(&mut registry.cached_models);
        *registry = new_registry;
        registry.cached_models = cached;
    }

    Ok(())
}

#[tauri::command]
pub async fn test_provider(
    provider_id: String,
    state: State<'_, AppState>,
) -> Result<bool, GlossError> {
    // Build provider config without holding lock across await
    let config = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let provider_type = ProviderType::from_str(&provider_id).unwrap_or(ProviderType::Ollama);

        let (base_url, api_key) = match provider_type {
            ProviderType::Ollama => (
                app_db
                    .get_setting("ollama_url")?
                    .unwrap_or_else(|| "http://localhost:11434".to_string()),
                None,
            ),
            ProviderType::OpenAI => (
                app_db
                    .get_setting("openai_base_url")?
                    .unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
                app_db.get_setting("openai_api_key")?,
            ),
            ProviderType::Anthropic => (
                app_db
                    .get_setting("anthropic_base_url")?
                    .unwrap_or_else(|| "https://api.anthropic.com/v1".to_string()),
                app_db.get_setting("anthropic_api_key")?,
            ),
            ProviderType::LlamaCpp => (
                app_db
                    .get_setting("llamacpp_url")?
                    .unwrap_or_else(|| "http://localhost:8080/v1".to_string()),
                None,
            ),
        };

        providers::ProviderConfig {
            provider_type,
            base_url,
            api_key,
        }
    };

    let provider = providers::build_provider(&config);
    provider.health_check().await
}

#[tauri::command]
pub async fn refresh_models(
    _provider_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<ModelInfo>, GlossError> {
    // Build all provider configs without holding lock across await
    let configs: Vec<providers::ProviderConfig> = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;

        let mut cfgs = Vec::new();

        // Ollama (always)
        cfgs.push(providers::ProviderConfig {
            provider_type: ProviderType::Ollama,
            base_url: app_db
                .get_setting("ollama_url")?
                .unwrap_or_else(|| "http://localhost:11434".to_string()),
            api_key: None,
        });

        // OpenAI (if key set)
        let openai_key = app_db.get_setting("openai_api_key")?.unwrap_or_default();
        if !openai_key.is_empty() {
            cfgs.push(providers::ProviderConfig {
                provider_type: ProviderType::OpenAI,
                base_url: app_db
                    .get_setting("openai_base_url")?
                    .unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
                api_key: Some(openai_key),
            });
        }

        // Anthropic (if key set)
        let anthropic_key = app_db.get_setting("anthropic_api_key")?.unwrap_or_default();
        if !anthropic_key.is_empty() {
            cfgs.push(providers::ProviderConfig {
                provider_type: ProviderType::Anthropic,
                base_url: app_db
                    .get_setting("anthropic_base_url")?
                    .unwrap_or_else(|| "https://api.anthropic.com/v1".to_string()),
                api_key: Some(anthropic_key),
            });
        }

        // LlamaCpp (if URL set)
        let llamacpp_url = app_db.get_setting("llamacpp_url")?.unwrap_or_default();
        if !llamacpp_url.is_empty() {
            cfgs.push(providers::ProviderConfig {
                provider_type: ProviderType::LlamaCpp,
                base_url: llamacpp_url,
                api_key: None,
            });
        }

        cfgs
    };

    // Fetch models from each provider (no locks held)
    let mut all_models = Vec::new();
    for config in &configs {
        let provider = providers::build_provider(config);
        match provider.list_models().await {
            Ok(models) => all_models.extend(models),
            Err(e) => tracing::warn!(
                provider = config.provider_type.as_str(),
                "Failed to refresh models: {}",
                e
            ),
        }
    }

    // Store in DB and update registry (lock held briefly, no await)
    {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let records = ModelRegistry::to_model_records(&all_models);

        // Group by provider and replace each
        for provider_type in &[
            ProviderType::Ollama,
            ProviderType::OpenAI,
            ProviderType::Anthropic,
            ProviderType::LlamaCpp,
        ] {
            let provider_records: Vec<ModelRecord> = records
                .iter()
                .filter(|r| r.provider_id == provider_type.as_str())
                .cloned()
                .collect();
            if !provider_records.is_empty() {
                app_db.replace_models(provider_type.as_str(), &provider_records)?;
            }
        }
    }

    {
        let mut registry = state
            .model_registry
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        registry.cached_models = all_models.clone();
    }

    Ok(all_models)
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

/// Check availability of external tools (ffmpeg, etc.)
#[tauri::command]
pub async fn check_external_tools() -> Result<HashMap<String, bool>, GlossError> {
    let mut tools = HashMap::new();

    tools.insert(
        "ffmpeg".to_string(),
        std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false),
    );

    tools.insert(
        "ffprobe".to_string(),
        std::process::Command::new("ffprobe")
            .arg("-version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false),
    );

    Ok(tools)
}
