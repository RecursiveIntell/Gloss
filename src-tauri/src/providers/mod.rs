pub mod anthropic;
pub mod llamacpp;
pub mod ollama;
pub mod openai;

use crate::db::app_db::{AppDb, ModelRecord};
use crate::error::GlossError;
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
    pub fn new(app_db: &AppDb) -> Result<Self, GlossError> {
        let ollama_url = app_db
            .get_setting("ollama_url")?
            .unwrap_or_else(|| "http://localhost:11434".to_string());
        let ollama = Some(ollama::OllamaProvider::new(&ollama_url));

        let openai = {
            let key = app_db.get_setting("openai_api_key")?.unwrap_or_default();
            let url = app_db
                .get_setting("openai_base_url")?
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
            if !key.is_empty() {
                Some(openai::OpenAIProvider::new(&url, &key))
            } else {
                None
            }
        };

        let anthropic = {
            let key = app_db.get_setting("anthropic_api_key")?.unwrap_or_default();
            let url = app_db
                .get_setting("anthropic_base_url")?
                .unwrap_or_else(|| "https://api.anthropic.com/v1".to_string());
            if !key.is_empty() {
                Some(anthropic::AnthropicProvider::new(&url, &key))
            } else {
                None
            }
        };

        let llamacpp = {
            let url = app_db.get_setting("llamacpp_url")?.unwrap_or_default();
            if !url.is_empty() {
                Some(llamacpp::LlamaCppProvider::new(&url))
            } else {
                None
            }
        };

        Ok(Self {
            ollama,
            openai,
            anthropic,
            llamacpp,
            cached_models: Vec::new(),
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
        // Default: try Ollama (backward compat for models not yet refreshed)
        self.ollama.as_ref().map(|p| p as &dyn LlmProvider)
    }

    /// Get a ProviderConfig for constructing a provider outside the lock.
    pub fn get_provider_config_for_model(
        &self,
        model_id: &str,
        app_db: &AppDb,
    ) -> Result<ProviderConfig, GlossError> {
        let provider_type = self
            .cached_models
            .iter()
            .find(|m| m.id == model_id)
            .map(|m| m.provider)
            .unwrap_or(ProviderType::Ollama);

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

        Ok(ProviderConfig {
            provider_type,
            base_url,
            api_key,
        })
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
            })
            .collect()
    }
}
