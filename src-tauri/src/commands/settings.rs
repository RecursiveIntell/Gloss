use crate::db::app_db::{ModelRecord, Provider};
use crate::error::GlossError;
use crate::providers::{self, ModelInfo, ModelRegistry, ProviderType};
use crate::state::AppState;
use std::collections::HashMap;
use tauri::State;

fn secret_setting_key(provider_type: ProviderType) -> Option<&'static str> {
    provider_type.api_key_setting_key()
}

fn provider_type_from_id(provider_id: &str) -> Result<ProviderType, GlossError> {
    ProviderType::from_str(provider_id.trim())
        .ok_or_else(|| GlossError::Config(format!("Unknown provider id '{provider_id}'")))
}

fn model_records_to_infos(records: Vec<ModelRecord>) -> Vec<ModelInfo> {
    records
        .into_iter()
        .filter(|m| m.available && !m.stale)
        .filter_map(|m| {
            ProviderType::from_str(&m.provider_id).map(|provider| ModelInfo {
                id: m.id,
                display_name: m.display_name,
                provider,
                parameter_size: m.parameter_size,
                context_window: m.context_window,
            })
        })
        .collect()
}

fn rebuild_model_registry(state: &AppState) -> Result<(), GlossError> {
    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    let new_registry = ModelRegistry::new(&app_db, &state.secret_store)?;
    let mut registry = state
        .model_registry
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    *registry = new_registry;
    Ok(())
}

#[tauri::command]
pub async fn get_providers(state: State<'_, AppState>) -> Result<Vec<Provider>, GlossError> {
    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    let mut providers = app_db.list_providers()?;
    drop(app_db);

    for provider in &mut providers {
        if let Some(provider_type) = ProviderType::from_str(&provider.id) {
            if let Some(secret_key) = secret_setting_key(provider_type) {
                provider.has_api_key = state.secret_store.contains(secret_key)?;
            }
        }
    }

    Ok(providers)
}

#[tauri::command]
pub async fn update_provider(
    id: String,
    enabled: bool,
    base_url: Option<String>,
    api_key: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), GlossError> {
    if let Some(provider_type) = ProviderType::from_str(&id) {
        if let Some(secret_key) = secret_setting_key(provider_type) {
            if let Some(api_key) = api_key.as_deref() {
                state.secret_store.set(secret_key, Some(api_key))?;
            }
        }
    }

    {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        app_db.update_provider(&id, enabled, base_url.as_deref(), None)?;
    }

    rebuild_model_registry(&state)?;

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
        let provider_type = provider_type_from_id(&provider_id)?;
        providers::provider_config_from_db(&app_db, &state.secret_store, provider_type)?
    };

    let provider = providers::build_provider(&config);
    provider.health_check().await
}

#[tauri::command]
pub async fn refresh_models(
    provider_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<ModelInfo>, GlossError> {
    let requested_provider = provider_id
        .as_deref()
        .map(provider_type_from_id)
        .transpose()?;

    // Build requested/enabled provider configs without holding lock across await
    let configs: Vec<providers::ProviderConfig> = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let providers = app_db.list_providers()?;

        let mut cfgs = Vec::new();
        let provider_types = if let Some(provider_type) = requested_provider {
            vec![provider_type]
        } else {
            vec![
                ProviderType::Ollama,
                ProviderType::OpenAI,
                ProviderType::Anthropic,
                ProviderType::LlamaCpp,
            ]
        };

        for provider_type in provider_types {
            let provider_row = providers
                .iter()
                .find(|provider| provider.id == provider_type.as_str());
            if provider_row.is_some_and(|provider| !provider.enabled) {
                if requested_provider.is_some() {
                    return Err(GlossError::Config(format!(
                        "Provider '{}' is disabled",
                        provider_type.as_str()
                    )));
                }
                continue;
            }

            let config =
                providers::provider_config_from_db(&app_db, &state.secret_store, provider_type)?;
            let missing_secret = matches!(
                provider_type,
                ProviderType::OpenAI | ProviderType::Anthropic
            ) && config.api_key.as_deref().unwrap_or("").is_empty();
            let missing_url = matches!(provider_type, ProviderType::LlamaCpp)
                && config.base_url.trim().is_empty();
            if missing_secret || missing_url {
                if requested_provider.is_some() {
                    return Err(GlossError::Config(format!(
                        "Provider '{}' is not fully configured",
                        provider_type.as_str()
                    )));
                }
                continue;
            }
            cfgs.push(config);
        }

        cfgs
    };

    // Fetch models from each provider (no locks held)
    let mut refreshed_models = Vec::new();
    let mut failed_providers = Vec::new();
    for config in &configs {
        let provider = providers::build_provider(config);
        match provider.list_models().await {
            Ok(models) => refreshed_models.extend(models),
            Err(e) => {
                tracing::warn!(
                    provider = config.provider_type.as_str(),
                    "Failed to refresh models: {}",
                    e
                );
                failed_providers.push((config.provider_type, e.to_string()));
            }
        }
    }

    // Store in DB and update registry (lock held briefly, no await)
    let available_models = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let records = ModelRegistry::to_model_records(&refreshed_models);

        for config in &configs {
            let provider_type = config.provider_type;
            let provider_records: Vec<ModelRecord> = records
                .iter()
                .filter(|r| r.provider_id == provider_type.as_str())
                .cloned()
                .collect();
            if failed_providers
                .iter()
                .any(|(failed_type, _)| *failed_type == provider_type)
            {
                continue;
            }
            app_db.replace_models(provider_type.as_str(), &provider_records)?;
        }

        for (provider_type, error) in &failed_providers {
            app_db.mark_models_unavailable(provider_type.as_str(), error)?;
        }

        model_records_to_infos(app_db.get_all_models()?)
    };

    {
        let mut registry = state
            .model_registry
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        registry.cached_models = available_models.clone();
    }

    Ok(available_models)
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
    let mut settings = app_db.get_settings()?;
    let providers = app_db.list_providers()?;
    drop(app_db);

    for (provider_id, setting_key, default_url) in [
        (
            ProviderType::Ollama.as_str(),
            "ollama_url",
            ProviderType::Ollama.default_base_url(),
        ),
        (
            ProviderType::OpenAI.as_str(),
            "openai_base_url",
            ProviderType::OpenAI.default_base_url(),
        ),
        (
            ProviderType::Anthropic.as_str(),
            "anthropic_base_url",
            ProviderType::Anthropic.default_base_url(),
        ),
        (
            ProviderType::LlamaCpp.as_str(),
            "llamacpp_url",
            ProviderType::LlamaCpp.default_base_url(),
        ),
    ] {
        let base_url = providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .and_then(|provider| provider.base_url.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(default_url);
        settings.insert(setting_key.to_string(), base_url.to_string());
    }

    for secret_key in ["openai_api_key", "anthropic_api_key"] {
        settings.insert(secret_key.to_string(), String::new());
        settings.insert(
            format!("{secret_key}_configured"),
            if state.secret_store.contains(secret_key)? {
                "1".to_string()
            } else {
                "0".to_string()
            },
        );
    }

    Ok(settings)
}

#[tauri::command]
pub async fn update_setting(
    key: String,
    value: String,
    state: State<'_, AppState>,
) -> Result<(), GlossError> {
    if matches!(
        key.as_str(),
        "ollama_url" | "openai_base_url" | "anthropic_base_url" | "llamacpp_url"
    ) {
        return Err(GlossError::Config(format!(
            "Provider URL setting '{key}' is read from the provider table; use update_provider"
        )));
    }

    if matches!(key.as_str(), "openai_api_key" | "anthropic_api_key") {
        state.secret_store.set(&key, Some(&value))?;
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        app_db.set_setting(&key, "")?;
        drop(app_db);
        rebuild_model_registry(&state)?;
        return Ok(());
    }

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

    for tool in ["ffmpeg", "ffprobe"] {
        let available = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            tokio::process::Command::new(tool)
                .arg("-version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status(),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .map(|status| status.success())
        .unwrap_or(false);
        tools.insert(tool.to_string(), available);
    }

    Ok(tools)
}
