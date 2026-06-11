use crate::db::app_db::{ModelRecord, Provider};
use crate::error::GlossError;
use crate::features::{self, FeatureFlagStatus};
use crate::memory::MemoryBackendStatus;
use crate::providers::{self, lan_local_providers_allowed, ModelInfo, ModelRegistry, ProviderType};
use crate::redaction::redact_path;
use crate::retrieval::source_scope::SourceScope;
use crate::state::AppState;
use crate::tool_invocation::{run_tool_status_receipt, ToolInvocationReceiptV1};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::State;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryProfile {
    GlossLocal,
    SemanticMemorySafe,
    SemanticMemoryStrict,
    SemanticMemoryTurboQuantSafe,
    SemanticMemoryTurboQuantStrict,
}

impl MemoryProfile {
    fn parse(input: &str) -> Result<Self, GlossError> {
        match input.trim().to_ascii_lowercase().as_str() {
            "gloss-local" => Ok(Self::GlossLocal),
            "semantic-memory" | "semantic-memory-hybrid" | "semantic-memory-safe" => {
                Ok(Self::SemanticMemorySafe)
            }
            "semantic-memory-strict" => Ok(Self::SemanticMemoryStrict),
            "semantic-memory-turbo-quant"
            | "semantic-memory+tq"
            | "semantic-memory-sm-tq"
            | "semantic-memory-turbo-quant-safe" => Ok(Self::SemanticMemoryTurboQuantSafe),
            "semantic-memory-turbo-quant-strict" => Ok(Self::SemanticMemoryTurboQuantStrict),
            other => Err(GlossError::Config(format!(
                "Unknown memory backend profile '{other}'"
            ))),
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::GlossLocal => "gloss-local",
            Self::SemanticMemorySafe => "semantic-memory-safe",
            Self::SemanticMemoryStrict => "semantic-memory-strict",
            Self::SemanticMemoryTurboQuantSafe => "semantic-memory-turbo-quant-safe",
            Self::SemanticMemoryTurboQuantStrict => "semantic-memory-turbo-quant-strict",
        }
    }

    fn is_strict(self) -> bool {
        matches!(
            self,
            Self::SemanticMemoryStrict | Self::SemanticMemoryTurboQuantStrict
        )
    }

    fn requires_semantic_memory(self) -> bool {
        !matches!(self, Self::GlossLocal)
    }

    fn requires_turbo_quant(self) -> bool {
        matches!(
            self,
            Self::SemanticMemoryTurboQuantSafe | Self::SemanticMemoryTurboQuantStrict
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryBackendProfileReceipt {
    pub profile: String,
    pub requested_backend: String,
    pub backend_used: String,
    pub strict_mode: bool,
    pub semantic_memory_auto_project: bool,
    pub turbo_quant_requested: bool,
    pub turbo_quant_active: bool,
    pub blocked: bool,
    pub next_action: Option<String>,
    pub blocking_reasons: Vec<String>,
    pub receipt_id: String,
    pub status: MemoryBackendStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct NativeFastEmbedDiagnostics {
    pub init_ok: bool,
    pub embed_one_ok: bool,
    pub dims: Option<usize>,
    pub cache_dir: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SemanticMemoryProviderDiagnostics {
    pub provider: String,
    pub dims: usize,
    pub model: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExternalToolAvailabilityReceipt {
    pub available: bool,
    pub receipt: ToolInvocationReceiptV1,
}

#[derive(Debug, Clone, Serialize)]
pub struct OptionalOllamaEmbeddingDiagnostics {
    pub configured: bool,
    pub url: Option<String>,
    pub embed_ok: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EmbeddingDiagnosticsReceipt {
    pub native_fastembed: NativeFastEmbedDiagnostics,
    pub semantic_memory_provider: SemanticMemoryProviderDiagnostics,
    pub optional_ollama: OptionalOllamaEmbeddingDiagnostics,
}

#[derive(Debug, Clone, Serialize)]
pub struct SemanticMemoryProfileStatus {
    pub compiled_semantic_memory: bool,
    pub compiled_turbo_quant: bool,
    pub experimental_enabled: bool,
    pub semantic_memory_flag_enabled: bool,
    pub turbo_quant_flag_enabled: bool,
    pub selected_backend: String,
    pub effective_backend: String,
    pub fallback_allowed: bool,
    pub strict_testing: bool,
    pub projection_summary: Option<crate::db::notebook_db::SemanticMemoryProjectionSummary>,
    pub turbo_quant_status: Option<crate::commands::sources::VectorArtifactStatus>,
    pub next_actions: Vec<String>,
    pub blocking_reasons: Vec<String>,
}

fn scoped_projection_summary(
    state: &AppState,
    notebook_id: Option<&str>,
    source_scope: Option<SourceScope>,
) -> Result<Option<crate::db::notebook_db::SemanticMemoryProjectionSummary>, GlossError> {
    let Some(notebook_id) = notebook_id else {
        return Ok(None);
    };
    state.with_notebook_db(notebook_id, |db| {
        let resolved_scope = if let Some(scope) = source_scope {
            let sources = db.list_sources()?;
            scope.resolve(&sources)
        } else {
            let sources = db.list_sources()?;
            SourceScope::All.resolve(&sources)
        };
        db.semantic_memory_projection_summary(notebook_id, &resolved_scope)
            .map(Some)
    })
}

fn projection_ready_for_strict(
    state: &AppState,
    notebook_id: Option<&str>,
) -> Result<(bool, Vec<String>), GlossError> {
    let Some(summary) = scoped_projection_summary(state, notebook_id, None)? else {
        return Ok((
            false,
            vec!["select a notebook before enabling strict semantic-memory".to_string()],
        ));
    };
    let mut reasons = Vec::new();
    if summary.chunk_bearing_sources == 0 || summary.total_chunks == 0 {
        reasons.push("selected scope has no chunk-bearing sources".to_string());
    }
    if summary.projection_required {
        reasons.push("projection required for selected source scope".to_string());
    }
    if summary.failed_sources > 0 {
        reasons.push("one or more source projections failed".to_string());
    }
    Ok((reasons.is_empty(), reasons))
}

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
    let provider_type = ProviderType::from_str(&id).ok_or_else(|| {
        GlossError::Other(format!(
            "Unrecognized provider type '{id}'; cannot update an unknown provider"
        ))
    })?;
    {
        let (allow_lan, current_url) = {
            let app_db = state
                .app_db
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            let lan = lan_local_providers_allowed(&app_db);
            // When the caller is not setting a new base_url, validate the
            // URL that is actually stored. Without this, a previous custom
            // (un-validated) URL would silently survive a no-op update even
            // if the LAN policy has since been tightened.
            let current = if base_url.is_none() {
                app_db.get_provider_url(&id).unwrap_or(None)
            } else {
                None
            };
            (lan, current)
        };
        let candidate_url = match base_url.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            Some(value) => value.to_string(),
            None => current_url.unwrap_or_else(|| provider_type.default_base_url().to_string()),
        };
        providers::validate_provider_base_url(provider_type, &candidate_url, allow_lan)?;
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
        app_db.update_provider(&id, enabled, base_url.as_deref())?;
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

    let provider = providers::build_provider(&config)?;
    provider.health_check().await
}

/// Test whether a specific model is available on the named provider.
/// Returns (health_ok, model_found, model_available).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderModelTestResult {
    pub provider_healthy: bool,
    pub model_found: bool,
    pub model_available: bool,
    pub model_list_error: Option<String>,
}

#[tauri::command]
pub async fn test_provider_model(
    provider_id: String,
    model_id: String,
    state: State<'_, AppState>,
) -> Result<ProviderModelTestResult, GlossError> {
    let config = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let provider_type = provider_type_from_id(&provider_id)?;
        providers::provider_config_from_db(&app_db, &state.secret_store, provider_type)?
    };

    let provider = providers::build_provider(&config)?;

    let provider_healthy = match provider.health_check().await {
        Ok(ok) => ok,
        Err(e) => {
            return Ok(ProviderModelTestResult {
                provider_healthy: false,
                model_found: false,
                model_available: false,
                model_list_error: Some(format!("health_check: {e}")),
            });
        }
    };

    let models = match provider.list_models().await {
        Ok(m) => m,
        Err(e) => {
            return Ok(ProviderModelTestResult {
                provider_healthy,
                model_found: false,
                model_available: false,
                model_list_error: Some(format!("list_models: {e}")),
            });
        }
    };

    let found = models.iter().find(|m| m.id == model_id);
    Ok(ProviderModelTestResult {
        provider_healthy,
        model_found: found.is_some(),
        model_available: found.is_some(), // list_models already filters unavailable
        model_list_error: None,
    })
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
        let provider = providers::build_provider(config)?;
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
pub async fn run_embedding_diagnostics(
    state: State<'_, AppState>,
) -> Result<EmbeddingDiagnosticsReceipt, GlossError> {
    let cache_dir = state.data_dir.join("models");
    let native_fastembed = match state.ensure_embedder(None) {
        Ok(()) => {
            let embed_result: Result<Vec<f32>, GlossError> = {
                let embedder_arc = {
                    let guard = state
                        .embedder
                        .read()
                        .map_err(|e| GlossError::Other(e.to_string()))?;
                    guard
                        .as_ref()
                        .ok_or_else(|| GlossError::Embedding("Embedder not initialized".into()))
                        .map(|arc| arc.clone())
                };
                match embedder_arc {
                    Ok(arc) => arc.embed_one("Gloss embedding diagnostics"),
                    Err(e) => Err(e),
                }
            };
            match embed_result {
                Ok(vector) => NativeFastEmbedDiagnostics {
                    init_ok: true,
                    embed_one_ok: true,
                    dims: Some(vector.len()),
                    cache_dir: redact_path(&cache_dir),
                    error: None,
                },
                Err(err) => NativeFastEmbedDiagnostics {
                    init_ok: true,
                    embed_one_ok: false,
                    dims: None,
                    cache_dir: redact_path(&cache_dir),
                    error: Some(err.to_string()),
                },
            }
        }
        Err(err) => NativeFastEmbedDiagnostics {
            init_ok: false,
            embed_one_ok: false,
            dims: None,
            cache_dir: redact_path(&cache_dir),
            error: Some(err.to_string()),
        },
    };

    let (provider, ollama_url) = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        (
            app_db
                .get_setting("semantic_memory_embedding_provider")?
                .unwrap_or_else(|| "ollama".to_string()),
            app_db.get_setting("semantic_memory_embedding_url")?,
        )
    };
    let provider = if provider.trim().eq_ignore_ascii_case("ollama") {
        "ollama"
    } else {
        "fastembed"
    };

    Ok(EmbeddingDiagnosticsReceipt {
        native_fastembed,
        semantic_memory_provider: SemanticMemoryProviderDiagnostics {
            provider: provider.to_string(),
            dims: 768,
            model: if provider == "fastembed" {
                "fastembed:NomicEmbedTextV15".to_string()
            } else {
                "nomic-embed-text".to_string()
            },
        },
        optional_ollama: OptionalOllamaEmbeddingDiagnostics {
            configured: provider == "ollama",
            url: ollama_url.filter(|url| provider == "ollama" && !url.trim().is_empty()),
            embed_ok: None,
        },
    })
}

#[tauri::command]
pub async fn update_setting(
    key: String,
    value: String,
    state: State<'_, AppState>,
) -> Result<(), GlossError> {
    /// Known setting keys that may be written via update_setting.
    const KNOWN_SETTINGS: &[&str] = &[
        "summary_mode",
        "summary_model",
        "vision_model",
        "theme",
        "default_model",
        "default_provider",
        "memory_backend",
        "memory_backend_fallback",
        "allow_lan_local_providers",
        "chunk_target_tokens",
        "semantic_memory_auto_project",
        "semantic_memory_turbo_quant_require_fresh_artifacts",
        "semantic_memory_strict_testing",
        "semantic_memory_embedding_provider",
        "semantic_memory_embedding_url",
        "semantic_memory_embedding_model",
        "semantic_memory_embedding_timeout_secs",
        "semantic_memory_search_timeout_ms",
        "semantic_memory_provekv_pool_candidates_enabled",
        "generation_temperature",
        "generation_top_p",
        "generation_top_k",
        "generation_min_p",
        "generation_repeat_penalty",
        // Feature flags
        "experimental_features_enabled",
        "feature_semantic_memory_preview_enabled",
        "feature_semantic_memory_turbo_quant_enabled",
        "feature_chat_diagnostics_enabled",
        "feature_provider_smoke_tools_enabled",
        "feature_advanced_retrieval_controls_enabled",
        "feature_index_replay_tools_enabled",
        "feature_package_release_panel_enabled",
        "feature_vision_jobs_enabled",
        "feature_video_import_enabled",
        "feature_background_summaries_enabled",
        "feature_external_tools_enabled",
        "feature_local_rag_enabled",
        "feature_source_scope_enabled",
        "fastembed_download_consent",
    ];

    let is_known = KNOWN_SETTINGS.contains(&key.as_str())
        || key.ends_with("_configured")
        || key.starts_with("openai_api_key")
        || key.starts_with("anthropic_api_key");

    if !is_known {
        return Err(GlossError::Config(format!(
            "Unrecognized setting key '{key}'"
        )));
    }

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
    features::validate_setting_update(&app_db, &key, &value)?;
    app_db.set_setting(&key, &value)?;
    features::apply_setting_update_side_effects(&app_db, &key, &value)?;
    Ok(())
}

#[tauri::command]
pub async fn get_feature_flags(
    state: State<'_, AppState>,
) -> Result<Vec<FeatureFlagStatus>, GlossError> {
    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    features::feature_flag_statuses(&app_db)
}

#[tauri::command]
pub async fn update_feature_flag(
    id: String,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<Vec<FeatureFlagStatus>, GlossError> {
    let app_db = state
        .app_db
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    features::update_feature_flag(&app_db, &id, enabled)?;
    features::feature_flag_statuses(&app_db)
}

#[tauri::command]
pub async fn set_memory_backend_profile(
    profile: String,
    notebook_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<MemoryBackendProfileReceipt, GlossError> {
    let profile = MemoryProfile::parse(&profile)?;
    let normalized = profile.id().to_string();
    let mut blocked = false;
    let mut next_action = None;
    let mut blocking_reasons = Vec::new();

    if profile.requires_semantic_memory() && !cfg!(feature = "semantic-memory-backend") {
        blocked = true;
        next_action = Some("rebuild_with_semantic_memory_backend".to_string());
        blocking_reasons.push(
            "semantic-memory profile requires the semantic-memory-backend build feature"
                .to_string(),
        );
    }

    if profile.requires_turbo_quant() && !cfg!(feature = "semantic-memory-turbo-quant") {
        blocked = true;
        next_action = Some("rebuild_with_semantic_memory_turbo_quant".to_string());
        blocking_reasons.push(
            "TurboQuant profile requires the semantic-memory-turbo-quant build feature".to_string(),
        );
    }

    if profile.is_strict() && !blocked {
        let (ready, reasons) = projection_ready_for_strict(&state, notebook_id.as_deref())?;
        if !ready {
            blocked = true;
            next_action = Some("run_projection_backfill".to_string());
            blocking_reasons = reasons;
        }
    }
    if profile == MemoryProfile::SemanticMemoryTurboQuantStrict && !blocked {
        if let Some(notebook_id) = notebook_id.clone() {
            let tq_status = crate::commands::sources::semantic_memory_vector_artifact_status(
                notebook_id,
                state.clone(),
            )
            .await?;
            if tq_status
                .candidate_backend
                .as_deref()
                .is_none_or(|backend| !backend.contains("turbo_quant"))
                || tq_status.artifact_generation_id.is_none()
                || tq_status.vector_artifact_manifest_digest.is_none()
                || !tq_status.exact_rerank
                || tq_status.exact_rerank_count == 0
                || tq_status.vector_artifact_missing_count > 0
                || tq_status.vector_artifact_stale_count > 0
            {
                blocked = true;
                next_action =
                    Some("run_retrieval_probe_and_rebuild_turbo_quant_artifacts".to_string());
                blocking_reasons.push(
                    "TurboQuant strict mode requires fresh artifact digest/generation and exact rerank proof"
                        .to_string(),
                );
            }
        } else {
            blocked = true;
            next_action = Some("select_notebook".to_string());
            blocking_reasons.push(
                "TurboQuant strict mode requires a notebook-scoped probe receipt".to_string(),
            );
        }
    }

    if blocked {
        let status = crate::commands::sources::memory_backend_status(notebook_id, state).await?;
        return Ok(MemoryBackendProfileReceipt {
            profile: normalized,
            requested_backend: status.backend_id.clone(),
            backend_used: status.backend_used.clone(),
            strict_mode: false,
            semantic_memory_auto_project: true,
            turbo_quant_requested: false,
            turbo_quant_active: false,
            blocked,
            next_action,
            blocking_reasons,
            receipt_id: uuid::Uuid::new_v4().to_string(),
            status,
        });
    }

    let (
        requested_backend,
        strict_mode,
        semantic_memory_auto_project,
        turbo_quant_requested,
        turbo_quant_active,
    ) = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;

        match profile {
            MemoryProfile::GlossLocal => {
                app_db.set_settings_atomically(&[
                    (features::EXPERIMENTAL_FEATURES_ENABLED, "false"),
                    (features::FEATURE_SEMANTIC_MEMORY_PREVIEW_ENABLED, "false"),
                    (
                        features::FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED,
                        "false",
                    ),
                    ("memory_backend", features::MEMORY_BACKEND_GLOSS_LOCAL),
                    ("memory_backend_fallback", "true"),
                    (features::SEMANTIC_MEMORY_AUTO_PROJECT, "false"),
                    (features::SEMANTIC_MEMORY_STRICT_TESTING, "false"),
                    (
                        features::SEMANTIC_MEMORY_TURBO_QUANT_REQUIRE_FRESH_ARTIFACTS,
                        "true",
                    ),
                    (
                        features::SEMANTIC_MEMORY_PROVEKV_POOL_CANDIDATES_ENABLED,
                        "false",
                    ),
                ])?;
            }
            MemoryProfile::SemanticMemorySafe => {
                app_db.set_settings_atomically(&[
                    (features::EXPERIMENTAL_FEATURES_ENABLED, "true"),
                    (features::FEATURE_SEMANTIC_MEMORY_PREVIEW_ENABLED, "true"),
                    (
                        features::FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED,
                        "false",
                    ),
                    (
                        "memory_backend",
                        features::MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW,
                    ),
                    ("memory_backend_fallback", "true"),
                    (features::SEMANTIC_MEMORY_AUTO_PROJECT, "true"),
                    (features::SEMANTIC_MEMORY_STRICT_TESTING, "false"),
                    (
                        features::SEMANTIC_MEMORY_TURBO_QUANT_REQUIRE_FRESH_ARTIFACTS,
                        "true",
                    ),
                    (
                        features::SEMANTIC_MEMORY_PROVEKV_POOL_CANDIDATES_ENABLED,
                        "false",
                    ),
                ])?;
            }
            MemoryProfile::SemanticMemoryStrict => {
                app_db.set_settings_atomically(&[
                    (features::EXPERIMENTAL_FEATURES_ENABLED, "true"),
                    (features::FEATURE_SEMANTIC_MEMORY_PREVIEW_ENABLED, "true"),
                    (
                        features::FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED,
                        "false",
                    ),
                    (
                        "memory_backend",
                        features::MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW,
                    ),
                    ("memory_backend_fallback", "false"),
                    (features::SEMANTIC_MEMORY_AUTO_PROJECT, "true"),
                    (features::SEMANTIC_MEMORY_STRICT_TESTING, "true"),
                    (
                        features::SEMANTIC_MEMORY_TURBO_QUANT_REQUIRE_FRESH_ARTIFACTS,
                        "true",
                    ),
                    (
                        features::SEMANTIC_MEMORY_PROVEKV_POOL_CANDIDATES_ENABLED,
                        "false",
                    ),
                ])?;
            }
            MemoryProfile::SemanticMemoryTurboQuantSafe => {
                app_db.set_settings_atomically(&[
                    (features::EXPERIMENTAL_FEATURES_ENABLED, "true"),
                    (features::FEATURE_SEMANTIC_MEMORY_PREVIEW_ENABLED, "true"),
                    (
                        features::FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED,
                        "true",
                    ),
                    (
                        "memory_backend",
                        features::MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW,
                    ),
                    ("memory_backend_fallback", "true"),
                    (features::SEMANTIC_MEMORY_AUTO_PROJECT, "true"),
                    (features::SEMANTIC_MEMORY_STRICT_TESTING, "false"),
                    (
                        features::SEMANTIC_MEMORY_TURBO_QUANT_REQUIRE_FRESH_ARTIFACTS,
                        "true",
                    ),
                    (
                        features::SEMANTIC_MEMORY_PROVEKV_POOL_CANDIDATES_ENABLED,
                        "false",
                    ),
                ])?;
            }
            MemoryProfile::SemanticMemoryTurboQuantStrict => {
                app_db.set_settings_atomically(&[
                    (features::EXPERIMENTAL_FEATURES_ENABLED, "true"),
                    (features::FEATURE_SEMANTIC_MEMORY_PREVIEW_ENABLED, "true"),
                    (
                        features::FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED,
                        "true",
                    ),
                    (
                        "memory_backend",
                        features::MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW,
                    ),
                    ("memory_backend_fallback", "false"),
                    (features::SEMANTIC_MEMORY_AUTO_PROJECT, "true"),
                    (features::SEMANTIC_MEMORY_STRICT_TESTING, "true"),
                    (
                        features::SEMANTIC_MEMORY_TURBO_QUANT_REQUIRE_FRESH_ARTIFACTS,
                        "true",
                    ),
                    (
                        features::SEMANTIC_MEMORY_PROVEKV_POOL_CANDIDATES_ENABLED,
                        "false",
                    ),
                ])?;
            }
        }

        let requested_backend = app_db
            .get_setting("memory_backend")?
            .unwrap_or_else(|| features::MEMORY_BACKEND_GLOSS_LOCAL.to_string());
        let strict_mode =
            !super::chat::setting_is_enabled(app_db.get_setting("memory_backend_fallback")?);
        let semantic_memory_auto_project = super::chat::setting_is_enabled(
            app_db.get_setting(features::SEMANTIC_MEMORY_AUTO_PROJECT)?,
        );
        let turbo_quant_requested = app_db
            .get_setting(features::FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED)?
            .as_deref()
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on" | "enabled"
                )
            })
            .unwrap_or(false);
        let turbo_quant_active = features::turbo_quant_active(&app_db)?;
        (
            requested_backend,
            strict_mode,
            semantic_memory_auto_project,
            turbo_quant_requested,
            turbo_quant_active,
        )
    };

    let status = crate::commands::sources::memory_backend_status(notebook_id, state).await?;
    Ok(MemoryBackendProfileReceipt {
        profile: normalized,
        requested_backend,
        backend_used: status.backend_used.clone(),
        strict_mode,
        semantic_memory_auto_project,
        turbo_quant_requested,
        turbo_quant_active,
        receipt_id: uuid::Uuid::new_v4().to_string(),
        blocked,
        next_action,
        blocking_reasons,
        status,
    })
}

#[tauri::command]
pub async fn get_semantic_memory_profile_status(
    notebook_id: Option<String>,
    source_scope: Option<SourceScope>,
    state: State<'_, AppState>,
) -> Result<SemanticMemoryProfileStatus, GlossError> {
    let (
        experimental_enabled,
        semantic_memory_flag_enabled,
        turbo_quant_flag_enabled,
        selected_backend,
        fallback_allowed,
        strict_testing,
    ) = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        (
            super::chat::setting_is_enabled(
                app_db.get_setting(features::EXPERIMENTAL_FEATURES_ENABLED)?,
            ),
            super::chat::setting_is_enabled(
                app_db.get_setting(features::FEATURE_SEMANTIC_MEMORY_PREVIEW_ENABLED)?,
            ),
            super::chat::setting_is_enabled(
                app_db.get_setting(features::FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED)?,
            ),
            app_db
                .get_setting("memory_backend")?
                .unwrap_or_else(|| features::MEMORY_BACKEND_GLOSS_LOCAL.to_string()),
            super::chat::setting_is_enabled(app_db.get_setting("memory_backend_fallback")?),
            super::chat::setting_is_enabled(
                app_db.get_setting(features::SEMANTIC_MEMORY_STRICT_TESTING)?,
            ),
        )
    };
    let projection_summary =
        scoped_projection_summary(&state, notebook_id.as_deref(), source_scope)?;
    let turbo_quant_status = if let Some(id) = notebook_id.clone() {
        Some(
            crate::commands::sources::semantic_memory_vector_artifact_status(id, state.clone())
                .await?,
        )
    } else {
        None
    };
    let backend_status =
        crate::commands::sources::memory_backend_status(notebook_id.clone(), state).await?;

    let mut next_actions = Vec::new();
    let mut blocking_reasons = Vec::new();
    if projection_summary
        .as_ref()
        .is_some_and(|summary| summary.projection_required)
    {
        next_actions.push("run_projection_backfill".to_string());
        blocking_reasons.push("projection required for selected source scope".to_string());
    }
    if turbo_quant_flag_enabled
        && turbo_quant_status.as_ref().is_some_and(|status| {
            status.artifact_generation_id.is_none()
                || status.vector_artifact_manifest_digest.is_none()
                || !status.exact_rerank
                || status.exact_rerank_count == 0
        })
    {
        next_actions.push("rebuild_turbo_quant_artifacts_and_run_retrieval_probe".to_string());
        blocking_reasons.push("TurboQuant strict proof is not fresh".to_string());
    }

    Ok(SemanticMemoryProfileStatus {
        compiled_semantic_memory: cfg!(feature = "semantic-memory-backend"),
        compiled_turbo_quant: cfg!(feature = "semantic-memory-turbo-quant"),
        experimental_enabled,
        semantic_memory_flag_enabled,
        turbo_quant_flag_enabled,
        selected_backend,
        effective_backend: backend_status.backend_used,
        fallback_allowed,
        strict_testing,
        projection_summary,
        turbo_quant_status,
        next_actions,
        blocking_reasons,
    })
}

/// Check availability of external tools (ffmpeg, etc.) with invocation receipts.
#[tauri::command]
pub async fn check_external_tools(
) -> Result<HashMap<String, ExternalToolAvailabilityReceipt>, GlossError> {
    let mut tools = HashMap::new();

    for tool in ["ffmpeg", "ffprobe"] {
        let receipt = run_tool_status_receipt(
            tool,
            "settings_availability_probe",
            &["-version"],
            std::time::Duration::from_secs(3),
        )
        .await
        .map_err(|e| GlossError::Other(e.to_string()))?;
        tools.insert(
            tool.to_string(),
            ExternalToolAvailabilityReceipt {
                available: receipt.success,
                receipt,
            },
        );
    }

    Ok(tools)
}

#[cfg(test)]
mod tests {
    use super::MemoryProfile;

    #[test]
    fn memory_profile_parser_normalizes_safe_and_strict_profiles() {
        assert_eq!(
            MemoryProfile::parse("semantic-memory").unwrap(),
            MemoryProfile::SemanticMemorySafe
        );
        assert_eq!(
            MemoryProfile::parse("semantic-memory-turbo-quant-safe").unwrap(),
            MemoryProfile::SemanticMemoryTurboQuantSafe
        );
        assert!(MemoryProfile::SemanticMemoryStrict.is_strict());
        assert!(MemoryProfile::SemanticMemoryTurboQuantStrict.requires_turbo_quant());
        assert_eq!(MemoryProfile::GlossLocal.id(), "gloss-local");
    }
}
