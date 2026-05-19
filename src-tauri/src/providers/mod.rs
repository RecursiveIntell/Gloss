pub mod anthropic;
pub mod llamacpp;
pub mod ollama;
pub mod openai;

use crate::db::app_db::{AppDb, ModelRecord, Provider};
use crate::error::GlossError;
use crate::provider_config_store::SecretStore;
use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderType {
    Ollama,
    OpenAI,
    Anthropic,
    LlamaCpp,
}

impl ProviderType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderType::Ollama => "ollama",
            ProviderType::OpenAI => "openai",
            ProviderType::Anthropic => "anthropic",
            ProviderType::LlamaCpp => "llamacpp",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "ollama" => Some(ProviderType::Ollama),
            "openai" => Some(ProviderType::OpenAI),
            "anthropic" => Some(ProviderType::Anthropic),
            "llamacpp" => Some(ProviderType::LlamaCpp),
            _ => None,
        }
    }

    pub fn api_key_setting_key(&self) -> Option<&'static str> {
        match self {
            ProviderType::OpenAI => Some("openai_api_key"),
            ProviderType::Anthropic => Some("anthropic_api_key"),
            ProviderType::Ollama | ProviderType::LlamaCpp => None,
        }
    }

    pub fn default_base_url(&self) -> &'static str {
        match self {
            ProviderType::Ollama => "http://localhost:11434",
            ProviderType::OpenAI => "https://api.openai.com/v1",
            ProviderType::Anthropic => "https://api.anthropic.com/v1",
            ProviderType::LlamaCpp => "http://localhost:8080/v1",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    pub provider: ProviderType,
    pub parameter_size: Option<String>,
    pub context_window: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    /// Optional base64-encoded images (for vision models).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub system_prompt: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub max_tokens: u32,
    pub temperature: f32,
    pub stream: bool,
    /// Ollama num_ctx: total context window size. When None, Ollama uses model default.
    pub num_ctx: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct ChatToken {
    pub token: String,
    pub done: bool,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// List available models from this provider.
    async fn list_models(&self) -> Result<Vec<ModelInfo>, GlossError>;

    /// Send a chat completion request, returning a token stream.
    async fn chat(
        &self,
        request: ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatToken, GlossError>> + Send>>, GlossError>;

    /// Test connectivity.
    async fn health_check(&self) -> Result<bool, GlossError>;

    /// Provider identifier.
    fn provider_type(&self) -> ProviderType;
}

/// Config needed to construct a provider outside the Mutex lock.
pub struct ProviderConfig {
    pub provider_type: ProviderType,
    pub base_url: String,
    pub api_key: Option<String>,
}

/// Construct a boxed LlmProvider from a config.
pub fn build_provider(config: &ProviderConfig) -> Box<dyn LlmProvider> {
    match config.provider_type {
        ProviderType::Ollama => Box::new(ollama::OllamaProvider::new(&config.base_url)),
        ProviderType::OpenAI => Box::new(openai::OpenAIProvider::new(
            &config.base_url,
            config.api_key.as_deref().unwrap_or(""),
        )),
        ProviderType::Anthropic => Box::new(anthropic::AnthropicProvider::new(
            &config.base_url,
            config.api_key.as_deref().unwrap_or(""),
        )),
        ProviderType::LlamaCpp => Box::new(llamacpp::LlamaCppProvider::new(&config.base_url)),
    }
}

fn provider_row<'a>(
    providers: &'a [Provider],
    provider_type: ProviderType,
) -> Option<&'a Provider> {
    providers
        .iter()
        .find(|provider| provider.id == provider_type.as_str())
}

fn provider_base_url(row: &Provider, provider_type: ProviderType) -> String {
    row.base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| provider_type.default_base_url())
        .to_string()
}

pub fn provider_config_from_db(
    app_db: &AppDb,
    secret_store: &SecretStore,
    provider_type: ProviderType,
) -> Result<ProviderConfig, GlossError> {
    let providers = app_db.list_providers()?;
    let row = provider_row(&providers, provider_type).ok_or_else(|| {
        GlossError::Config(format!(
            "Provider '{}' is missing from the provider table",
            provider_type.as_str()
        ))
    })?;
    if !row.enabled {
        return Err(GlossError::Config(format!(
            "Provider '{}' is disabled",
            provider_type.as_str()
        )));
    }

    Ok(ProviderConfig {
        provider_type,
        base_url: provider_base_url(row, provider_type),
        api_key: provider_type
            .api_key_setting_key()
            .map(|key| secret_store.get(key))
            .transpose()?
            .flatten(),
    })
}

/// Registry of all configured LLM providers and cached models.
pub struct ModelRegistry {
    pub ollama: Option<ollama::OllamaProvider>,
    pub openai: Option<openai::OpenAIProvider>,
    pub anthropic: Option<anthropic::AnthropicProvider>,
    pub llamacpp: Option<llamacpp::LlamaCppProvider>,
    pub cached_models: Vec<ModelInfo>,
}

impl ModelRegistry {
    /// Create registry from app database config.
    pub fn new(app_db: &AppDb, secret_store: &SecretStore) -> Result<Self, GlossError> {
        let providers = app_db.list_providers()?;
        let ollama = provider_row(&providers, ProviderType::Ollama)
            .filter(|row| row.enabled)
            .map(|row| ollama::OllamaProvider::new(&provider_base_url(row, ProviderType::Ollama)));

        let openai = match provider_row(&providers, ProviderType::OpenAI) {
            Some(row) if row.enabled => {
                let key = secret_store.get("openai_api_key")?.unwrap_or_default();
                if key.is_empty() {
                    None
                } else {
                    Some(openai::OpenAIProvider::new(
                        &provider_base_url(row, ProviderType::OpenAI),
                        &key,
                    ))
                }
            }
            _ => None,
        };

        let anthropic = match provider_row(&providers, ProviderType::Anthropic) {
            Some(row) if row.enabled => {
                let key = secret_store.get("anthropic_api_key")?.unwrap_or_default();
                if key.is_empty() {
                    None
                } else {
                    Some(anthropic::AnthropicProvider::new(
                        &provider_base_url(row, ProviderType::Anthropic),
                        &key,
                    ))
                }
            }
            _ => None,
        };

        let llamacpp = provider_row(&providers, ProviderType::LlamaCpp)
            .filter(|row| row.enabled)
            .map(|row| {
                llamacpp::LlamaCppProvider::new(&provider_base_url(row, ProviderType::LlamaCpp))
            });

        let cached_models = app_db
            .get_all_models()?
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
            .collect();

        Ok(Self {
            ollama,
            openai,
            anthropic,
            llamacpp,
            cached_models,
        })
    }

    /// Get the provider for a given model ID (looks up which provider owns it).
    pub fn get_provider_for_model(&self, model_id: &str) -> Option<&dyn LlmProvider> {
        // Check cached models to find which provider owns this model
        for m in &self.cached_models {
            if m.id == model_id {
                return match m.provider {
                    ProviderType::Ollama => self.ollama.as_ref().map(|p| p as &dyn LlmProvider),
                    ProviderType::OpenAI => self.openai.as_ref().map(|p| p as &dyn LlmProvider),
                    ProviderType::Anthropic => {
                        self.anthropic.as_ref().map(|p| p as &dyn LlmProvider)
                    }
                    ProviderType::LlamaCpp => self.llamacpp.as_ref().map(|p| p as &dyn LlmProvider),
                };
            }
        }
        None
    }

    /// Get a ProviderConfig for constructing a provider outside the lock.
    pub fn get_provider_config_for_model(
        &self,
        model_id: &str,
        app_db: &AppDb,
        secret_store: &SecretStore,
    ) -> Result<ProviderConfig, GlossError> {
        let selected_provider = app_db
            .get_setting("default_provider")?
            .and_then(|provider_id| ProviderType::from_str(provider_id.trim()));
        let candidates = self
            .cached_models
            .iter()
            .filter(|m| m.id == model_id)
            .collect::<Vec<_>>();
        let provider_type = if let Some(provider_type) = selected_provider {
            candidates
                .iter()
                .find(|m| m.provider == provider_type)
                .map(|m| m.provider)
        } else if candidates.len() == 1 {
            candidates.first().map(|m| m.provider)
        } else {
            None
        }
        .ok_or_else(|| {
            GlossError::Config(format!(
                "Selected chat model '{model_id}' is not available in the model registry"
            ))
        })?;

        provider_config_from_db(app_db, secret_store, provider_type)
    }

    /// Refresh models from all enabled providers.
    pub async fn refresh_all(&mut self) -> Result<Vec<ModelInfo>, GlossError> {
        let mut all_models = Vec::new();
        if let Some(ref ollama) = self.ollama {
            match ollama.list_models().await {
                Ok(models) => all_models.extend(models),
                Err(e) => tracing::warn!("Failed to refresh Ollama models: {}", e),
            }
        }
        if let Some(ref openai) = self.openai {
            match openai.list_models().await {
                Ok(models) => all_models.extend(models),
                Err(e) => tracing::warn!("Failed to refresh OpenAI models: {}", e),
            }
        }
        if let Some(ref anthropic) = self.anthropic {
            match anthropic.list_models().await {
                Ok(models) => all_models.extend(models),
                Err(e) => tracing::warn!("Failed to refresh Anthropic models: {}", e),
            }
        }
        if let Some(ref llamacpp) = self.llamacpp {
            match llamacpp.list_models().await {
                Ok(models) => all_models.extend(models),
                Err(e) => tracing::warn!("Failed to refresh llama.cpp models: {}", e),
            }
        }
        self.cached_models = all_models.clone();
        Ok(all_models)
    }

    /// Get cached models.
    pub fn get_cached_models(&self) -> &[ModelInfo] {
        &self.cached_models
    }

    /// Convert ModelInfo to ModelRecord for DB storage.
    pub fn to_model_records(models: &[ModelInfo]) -> Vec<ModelRecord> {
        models
            .iter()
            .map(|m| ModelRecord {
                id: m.id.clone(),
                provider_id: m.provider.as_str().to_string(),
                display_name: m.display_name.clone(),
                parameter_size: m.parameter_size.clone(),
                context_window: m.context_window,
                capabilities: None,
                available: true,
                stale: false,
                last_error: None,
            })
            .collect()
    }
}
