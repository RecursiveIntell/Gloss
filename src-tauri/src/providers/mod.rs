pub mod ollama;

use crate::db::app_db::{AppDb, ModelRecord};
use crate::error::GlossError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use futures::Stream;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderType {
    Ollama,
    OpenAI,
    Anthropic,
}

impl ProviderType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderType::Ollama => "ollama",
            ProviderType::OpenAI => "openai",
            ProviderType::Anthropic => "anthropic",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "ollama" => Some(ProviderType::Ollama),
            "openai" => Some(ProviderType::OpenAI),
            "anthropic" => Some(ProviderType::Anthropic),
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

/// Registry of all configured LLM providers and cached models.
pub struct ModelRegistry {
    pub ollama: Option<ollama::OllamaProvider>,
    pub cached_models: Vec<ModelInfo>,
}

impl ModelRegistry {
    /// Create registry from app database config.
    pub fn new(app_db: &AppDb) -> Result<Self, GlossError> {
        let url = app_db
            .get_setting("ollama_url")?
            .unwrap_or_else(|| "http://localhost:11434".to_string());

        let ollama = Some(ollama::OllamaProvider::new(&url));

        Ok(Self {
            ollama,
            cached_models: Vec::new(),
        })
    }

    /// Get the provider for a given model ID (looks up which provider owns it).
    pub fn get_provider_for_model(&self, _model_id: &str) -> Option<&dyn LlmProvider> {
        // For now, all models are Ollama
        if let Some(ref ollama) = self.ollama {
            return Some(ollama as &dyn LlmProvider);
        }
        None
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
