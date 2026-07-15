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
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const LOCAL_EGRESS_HOSTS: &[&str] = &["localhost", "127.0.0.1", "::1"];
const OPENAI_EGRESS_HOST: &str = "api.openai.com";
const ANTHROPIC_EGRESS_HOST: &str = "api.anthropic.com";

const ALLOW_LAN_LOCAL_PROVIDERS_KEY: &str = "allow_lan_local_providers";
const ALLOW_CUSTOM_CLOUD_ENDPOINTS_KEY: &str = "allow_custom_cloud_endpoints";

fn setting_is_enabled(value: Option<String>) -> bool {
    value
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on" | "enabled"
            )
        })
        .unwrap_or(false)
}

fn read_setting_flag(app_db: &crate::db::app_db::AppDb, key: &str) -> bool {
    setting_is_enabled(app_db.get_setting(key).ok().flatten())
}

fn is_rfc1918_host(host: &str) -> bool {
    // 10.0.0.0/8
    if host.starts_with("10.") || host.starts_with("10:") {
        return true;
    }
    // 172.16.0.0/12
    if host.starts_with("172.") {
        let parts: Vec<&str> = host.split('.').collect();
        if parts.len() >= 2 {
            if let Ok(second_octet) = parts[1].parse::<u8>() {
                if (16..=31).contains(&second_octet) {
                    return true;
                }
            }
        }
    }
    // 192.168.0.0/16
    if host.starts_with("192.168.") {
        return true;
    }
    // IPv6 private: fc00::/7
    if host.starts_with("fc")
        || host.starts_with("fd")
        || host.starts_with("fc00")
        || host.starts_with("fd00")
    {
        return true;
    }
    false
}

pub fn lan_local_providers_allowed(app_db: &crate::db::app_db::AppDb) -> bool {
    read_setting_flag(app_db, ALLOW_LAN_LOCAL_PROVIDERS_KEY)
}

pub fn custom_cloud_endpoints_allowed(app_db: &crate::db::app_db::AppDb) -> bool {
    read_setting_flag(app_db, ALLOW_CUSTOM_CLOUD_ENDPOINTS_KEY)
}

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
    pub top_p: Option<f32>,
    pub top_k: Option<i64>,
    pub min_p: Option<f32>,
    pub repeat_penalty: Option<f32>,
    pub stream: bool,
    /// Ollama num_ctx: total context window size. When None, Ollama uses model default.
    pub num_ctx: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct ChatToken {
    pub token: String,
    pub done: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct LlmPhaseTimeouts {
    pub provider_start: Duration,
    pub first_token: Duration,
    pub stream_idle: Duration,
}

impl Default for LlmPhaseTimeouts {
    fn default() -> Self {
        Self {
            provider_start: Duration::from_secs(180),
            first_token: Duration::from_secs(168),
            stream_idle: Duration::from_secs(84),
        }
    }
}

/// Stable identity snapshot for one LLM-affecting operation. It deliberately
/// travels with `LlmExecutionContext` rather than becoming another mutable
/// runtime registry; the chat terminal emitter remains the sole terminal
/// event guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // fields are part of the audit/receipt surface; not all are read in every build
pub enum LlmOperationKind {
    Chat,
    Summary,
    Vision,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // populated for receipt/audit serialization; not every build path reads each field
pub struct LlmOperationContext {
    pub kind: LlmOperationKind,
    pub notebook_id: String,
    pub conversation_id: Option<String>,
    pub output_id: Option<String>,
    pub source_id: Option<String>,
    pub epoch: u64,
    pub attempt_id: String,
    pub provider_snapshot: String,
    pub model_snapshot: String,
    pub receipt_id: String,
}

impl LlmOperationContext {
    #[allow(clippy::too_many_arguments)]
    pub fn chat(
        notebook_id: impl Into<String>,
        conversation_id: impl Into<String>,
        output_id: impl Into<String>,
        epoch: u64,
        attempt_id: impl Into<String>,
        provider_snapshot: impl Into<String>,
        model_snapshot: impl Into<String>,
        receipt_id: impl Into<String>,
    ) -> Self {
        Self {
            kind: LlmOperationKind::Chat,
            notebook_id: notebook_id.into(),
            conversation_id: Some(conversation_id.into()),
            output_id: Some(output_id.into()),
            source_id: None,
            epoch,
            attempt_id: attempt_id.into(),
            provider_snapshot: provider_snapshot.into(),
            model_snapshot: model_snapshot.into(),
            receipt_id: receipt_id.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlmExecutionContext {
    pub cancellation: CancellationToken,
    pub timeouts: LlmPhaseTimeouts,
    pub attempt_id: Option<String>,
    pub operation: Option<LlmOperationContext>,
}

impl LlmExecutionContext {
    pub fn new(cancellation: CancellationToken, timeouts: LlmPhaseTimeouts) -> Self {
        Self {
            cancellation,
            timeouts,
            attempt_id: None,
            operation: None,
        }
    }

    pub fn default_with_token(cancellation: CancellationToken) -> Self {
        Self::new(cancellation, LlmPhaseTimeouts::default())
    }

    pub fn uncancellable() -> Self {
        Self::default_with_token(CancellationToken::new())
    }

    pub fn with_attempt_id(mut self, attempt_id: impl Into<String>) -> Self {
        self.attempt_id = Some(attempt_id.into());
        self
    }

    pub fn with_operation(mut self, operation: LlmOperationContext) -> Self {
        self.attempt_id = Some(operation.attempt_id.clone());
        self.operation = Some(operation);
        self
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    pub fn check_cancelled(&self, provider: &str, phase: &str) -> Result<(), GlossError> {
        if self.is_cancelled() {
            Err(provider_cancelled_error(
                provider,
                phase,
                self.attempt_id.as_deref(),
            ))
        } else {
            Ok(())
        }
    }
}

pub fn provider_cancelled_error(
    provider: &str,
    phase: &str,
    attempt_id: Option<&str>,
) -> GlossError {
    let attempt_detail = attempt_id
        .map(|attempt_id| format!(" attempt_id={attempt_id}"))
        .unwrap_or_default();
    GlossError::Provider {
        provider: provider.to_string(),
        source: anyhow::anyhow!("provider request cancelled during {phase}{attempt_detail}"),
    }
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// List available models from this provider.
    async fn list_models(&self) -> Result<Vec<ModelInfo>, GlossError>;

    /// Send a chat completion request, returning a token stream.
    async fn chat(
        &self,
        request: ChatRequest,
        ctx: LlmExecutionContext,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkScopeReceiptV1 {
    pub schema: String,
    pub provider: String,
    pub base_url: String,
    pub host: String,
    pub egress_class: String,
    pub policy: String,
    pub cloud_opt_in_required: bool,
    pub lan_opt_in_applied: bool,
}

pub fn validate_provider_base_url(
    provider_type: ProviderType,
    base_url: &str,
    allow_lan: bool,
    allow_custom_cloud_endpoints: bool,
) -> Result<NetworkScopeReceiptV1, GlossError> {
    validate_base_url_inner(
        provider_type,
        base_url,
        allow_lan,
        allow_custom_cloud_endpoints,
        false,
    )
}

/// Validate an embedding URL against the same LAN/loopback policy as provider URLs.
/// Embedding endpoints (Ollama for nomic-embed-text, etc.) must obey the same
/// network scope restrictions: loopback always allowed, LAN requires opt-in,
/// public/cloud always rejected.
pub fn validate_embedding_url(
    base_url: &str,
    allow_lan: bool,
) -> Result<NetworkScopeReceiptV1, GlossError> {
    // Embedding endpoints are treated as local providers (Ollama/LlamaCpp)
    // — they must be loopback or LAN-with-opt-in.
    validate_base_url_inner(ProviderType::Ollama, base_url, allow_lan, false, true)
}

/// Build a shared reqwest::Client suitable for all supported providers.
///
/// A single client is reused across provider instances and chat turns so the
/// connection pool survives across uses and avoids repeated TCP handshakes.
pub fn build_shared_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .read_timeout(std::time::Duration::from_secs(90))
        .timeout(std::time::Duration::from_secs(300))
        .pool_max_idle_per_host(8)
        .tcp_keepalive(std::time::Duration::from_secs(60))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

fn validate_base_url_inner(
    provider_type: ProviderType,
    base_url: &str,
    allow_lan: bool,
    allow_custom_cloud_endpoints: bool,
    is_embedding: bool,
) -> Result<NetworkScopeReceiptV1, GlossError> {
    let label = if is_embedding {
        "Embedding endpoint"
    } else {
        "Provider"
    };
    let parsed = reqwest::Url::parse(base_url.trim()).map_err(|e| {
        GlossError::Config(format!(
            "{label} '{}' base URL is invalid: {e}",
            provider_type.as_str()
        ))
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(GlossError::Config(format!(
            "{label} '{}' base URL must use http or https",
            provider_type.as_str()
        )));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(GlossError::Config(format!(
            "{label} '{}' base URL must not include credentials",
            provider_type.as_str()
        )));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(GlossError::Config(format!(
            "{label} '{}' base URL must not include query strings or fragments",
            provider_type.as_str()
        )));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| {
            GlossError::Config(format!(
                "{label} '{}' base URL must include a host",
                provider_type.as_str()
            ))
        })?
        .to_ascii_lowercase();
    let is_loopback = LOCAL_EGRESS_HOSTS.contains(&host.as_str());
    let is_lan = is_rfc1918_host(&host);
    let (allowed, egress_class, policy, cloud_opt_in_required, lan_opt_in_applied) =
        match provider_type {
            ProviderType::Ollama | ProviderType::LlamaCpp => {
                if is_loopback {
                    (
                        true,
                        "local_loopback",
                        "local providers default to loopback endpoints",
                        false,
                        false,
                    )
                } else if is_lan && allow_lan {
                    (true, "local_lan", "LAN local providers permitted by operator opt-in (allow_lan_local_providers=true)", false, true)
                } else if is_lan {
                    (false, "lan_rejected", "LAN local provider URL rejected; set allow_lan_local_providers=true to permit RFC1918 LAN endpoints", false, false)
                } else {
                    (false, "public_rejected", "local providers are restricted to loopback (or LAN with opt-in); public IPs are always rejected", false, false)
                }
            }
            ProviderType::OpenAI => {
                if allow_custom_cloud_endpoints {
                    (
                        parsed.scheme() == "https",
                        "custom_cloud",
                        "custom cloud endpoint permitted by operator opt-in",
                        false,
                        false,
                    )
                } else {
                    (
                    parsed.scheme() == "https" && host == OPENAI_EGRESS_HOST,
                    "cloud_default",
                    "OpenAI provider is restricted to https://api.openai.com without custom endpoint opt-in",
                    true,
                    false,
                )
                }
            }
            ProviderType::Anthropic => {
                if allow_custom_cloud_endpoints {
                    (
                        parsed.scheme() == "https",
                        "custom_cloud",
                        "custom cloud endpoint permitted by operator opt-in",
                        false,
                        false,
                    )
                } else {
                    (
                    parsed.scheme() == "https" && host == ANTHROPIC_EGRESS_HOST,
                    "cloud_default",
                    "Anthropic provider is restricted to https://api.anthropic.com without custom endpoint opt-in",
                    true,
                    false,
                )
                }
            }
        };
    if !allowed {
        return Err(GlossError::Config(format!(
            "Provider '{}' base URL '{}' is outside the active NetworkScopePolicy: {}",
            provider_type.as_str(),
            redact_url_for_error(base_url),
            policy
        )));
    }
    Ok(NetworkScopeReceiptV1 {
        schema: "NetworkScopeReceiptV1".to_string(),
        provider: provider_type.as_str().to_string(),
        base_url: parsed.as_str().trim_end_matches('/').to_string(),
        host,
        egress_class: egress_class.to_string(),
        policy: policy.to_string(),
        cloud_opt_in_required,
        lan_opt_in_applied,
    })
}

pub fn sanitize_provider_error_body(body: &str) -> String {
    let body = body
        .replace(['\n', '\r'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = String::new();
    let mut redact_next = false;
    for token in body.split(' ') {
        let lower = token.to_ascii_lowercase();
        let redacts_following = lower.contains("authorization")
            || lower == "bearer"
            || lower.contains("api_key")
            || lower.contains("apikey")
            || lower == "token"
            || lower == "secret";
        let redacted = redact_next
            || redacts_following
            || lower.contains("token=")
            || lower.contains("secret=")
            || lower.contains("api_key=")
            || lower.contains("apikey=")
            || token.starts_with("sk-")
            || token.starts_with("Bearer");
        if !out.is_empty() {
            out.push(' ');
        }
        if redacted {
            out.push_str("[redacted]");
        } else {
            out.push_str(token);
        }
        redact_next = redacts_following;
        if out.len() >= 240 {
            out.truncate(240);
            out.push_str("...");
            break;
        }
    }
    let out = crate::redaction::redact_json_embedded_secrets(&out);
    if out.is_empty() {
        "[empty error body]".to_string()
    } else {
        out
    }
}

pub fn provider_http_error(provider: &str, status: reqwest::StatusCode, body: &str) -> GlossError {
    GlossError::Provider {
        provider: provider.to_string(),
        source: anyhow::anyhow!(
            "HTTP {}: sanitized_body={}",
            status,
            sanitize_provider_error_body(body)
        ),
    }
}

fn redact_url_for_error(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut parsed) => {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.set_query(None);
            parsed.set_fragment(None);
            // Redact LAN/private host details: if the host is not loopback,
            // truncate IP to first octet or domain to first label, to avoid
            // leaking internal network topology in log/error output.
            if let Some(host) = parsed.host_str() {
                if host != "localhost" && host != "127.0.0.1" && host != "::1" {
                    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
                        match ip {
                            std::net::IpAddr::V4(v4) => {
                                let octets = v4.octets();
                                let _ = parsed.set_host(Some(&format!(
                                    "{}.x.x.x:{}",
                                    octets[0],
                                    parsed.port().unwrap_or(0)
                                )));
                            }
                            std::net::IpAddr::V6(v6) => {
                                let segments = v6.segments();
                                let _ = parsed.set_host(Some(&format!(
                                    "[{}:x:x:x:...]:{}",
                                    segments[0],
                                    parsed.port().unwrap_or(0)
                                )));
                            }
                        }
                    } else {
                        // Domain name: keep only first label
                        if let Some(dot_pos) = host.find('.') {
                            let _ = parsed.set_host(Some(&format!(
                                "{}.***:{}",
                                &host[..dot_pos],
                                parsed.port().unwrap_or(0)
                            )));
                        }
                    }
                }
            }
            parsed.as_str().to_string()
        }
        Err(_) => "[invalid-url]".to_string(),
    }
}

/// Construct a boxed LlmProvider from a config.
pub fn build_provider(config: &ProviderConfig) -> Result<Box<dyn LlmProvider>, GlossError> {
    let shared_client = build_shared_client();
    match config.provider_type {
        ProviderType::OpenAI | ProviderType::Anthropic => {
            let api_key = config
                .api_key
                .as_deref()
                .map(|k| k.trim())
                .filter(|k| !k.is_empty())
                .ok_or_else(|| {
                    GlossError::Other(
                        "API key not configured for this provider. Add an API key in Settings."
                            .into(),
                    )
                })?;
            match config.provider_type {
                ProviderType::OpenAI => Ok(Box::new(openai::OpenAIProvider::new(
                    &config.base_url,
                    api_key,
                    shared_client.clone(),
                ))),
                ProviderType::Anthropic => Ok(Box::new(anthropic::AnthropicProvider::new(
                    &config.base_url,
                    api_key,
                    shared_client.clone(),
                ))),
                _ => unreachable!(),
            }
        }
        ProviderType::Ollama => Ok(Box::new(ollama::OllamaProvider::new(
            &config.base_url,
            shared_client.clone(),
        ))),
        ProviderType::LlamaCpp => Ok(Box::new(llamacpp::LlamaCppProvider::new(
            &config.base_url,
            shared_client.clone(),
        ))),
    }
}

fn provider_row(providers: &[Provider], provider_type: ProviderType) -> Option<&Provider> {
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

fn validated_provider_base_url(
    row: &Provider,
    provider_type: ProviderType,
    allow_lan: bool,
    allow_custom_cloud_endpoints: bool,
) -> Result<String, GlossError> {
    let base_url = provider_base_url(row, provider_type);
    Ok(validate_provider_base_url(
        provider_type,
        &base_url,
        allow_lan,
        allow_custom_cloud_endpoints,
    )?
    .base_url)
}

pub fn provider_config_from_db(
    app_db: &AppDb,
    secret_store: &SecretStore,
    provider_type: ProviderType,
) -> Result<ProviderConfig, GlossError> {
    let allow_lan = lan_local_providers_allowed(app_db);
    let allow_custom_cloud_endpoints = custom_cloud_endpoints_allowed(app_db);
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
        base_url: validated_provider_base_url(
            row,
            provider_type,
            allow_lan,
            allow_custom_cloud_endpoints,
        )?,
        api_key: provider_type
            .api_key_setting_key()
            .map(|key| secret_store.get(key))
            .transpose()?
            .flatten(),
    })
}

/// Registry of all configured LLM providers and cached models.
#[allow(dead_code)]
pub struct ModelRegistry {
    pub ollama: Option<ollama::OllamaProvider>,
    pub openai: Option<openai::OpenAIProvider>,
    pub anthropic: Option<anthropic::AnthropicProvider>,
    pub llamacpp: Option<llamacpp::LlamaCppProvider>,
    pub cached_models: Vec<ModelInfo>,
}

#[allow(dead_code)]
impl ModelRegistry {
    /// Create registry from app database config.
    pub fn new(app_db: &AppDb, secret_store: &SecretStore) -> Result<Self, GlossError> {
        let allow_lan = lan_local_providers_allowed(app_db);
        let allow_custom_cloud_endpoints = custom_cloud_endpoints_allowed(app_db);
        let providers = app_db.list_providers()?;
        let shared_client = build_shared_client();
        let ollama = provider_row(&providers, ProviderType::Ollama)
            .filter(|row| row.enabled)
            .and_then(|row| {
                validated_provider_base_url(
                    row,
                    ProviderType::Ollama,
                    allow_lan,
                    allow_custom_cloud_endpoints,
                )
                .ok()
            })
            .map(|base_url| ollama::OllamaProvider::new(&base_url, shared_client.clone()));

        let openai = match provider_row(&providers, ProviderType::OpenAI) {
            Some(row) if row.enabled => {
                let key = secret_store.get("openai_api_key")?.unwrap_or_default();
                if key.is_empty() {
                    None
                } else {
                    Some(openai::OpenAIProvider::new(
                        &validated_provider_base_url(
                            row,
                            ProviderType::OpenAI,
                            allow_lan,
                            allow_custom_cloud_endpoints,
                        )?,
                        &key,
                        shared_client.clone(),
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
                        &validated_provider_base_url(
                            row,
                            ProviderType::Anthropic,
                            allow_lan,
                            allow_custom_cloud_endpoints,
                        )?,
                        &key,
                        shared_client.clone(),
                    ))
                }
            }
            _ => None,
        };

        let llamacpp = provider_row(&providers, ProviderType::LlamaCpp)
            .filter(|row| row.enabled)
            .and_then(|row| {
                validated_provider_base_url(
                    row,
                    ProviderType::LlamaCpp,
                    allow_lan,
                    allow_custom_cloud_endpoints,
                )
                .ok()
            })
            .map(|base_url| llamacpp::LlamaCppProvider::new(&base_url, shared_client.clone()));

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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::{stream, StreamExt};
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };
    use tokio::time::{sleep, Duration};

    #[derive(Clone)]
    enum MockProviderMode {
        NeverStarts,
        SlowStart {
            delay_ms: u64,
        },
        SlowFirstToken {
            delay_ms: u64,
        },
        IdleAfterChunks {
            chunks: Vec<&'static str>,
            idle_ms: u64,
        },
        LateChunksAfterCancel,
        IncompleteEof,
        DoneNoContent,
        Normal {
            chunks: Vec<&'static str>,
        },
    }

    struct HarnessMockProvider {
        mode: MockProviderMode,
        active_requests: Arc<AtomicUsize>,
        cancellation_observed: Arc<AtomicBool>,
    }

    impl HarnessMockProvider {
        fn new(mode: MockProviderMode) -> Self {
            Self {
                mode,
                active_requests: Arc::new(AtomicUsize::new(0)),
                cancellation_observed: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    struct MockStreamState {
        mode: MockProviderMode,
        step: usize,
        ctx: LlmExecutionContext,
        active_requests: Arc<AtomicUsize>,
        cancellation_observed: Arc<AtomicBool>,
    }

    impl Drop for MockStreamState {
        fn drop(&mut self) {
            self.active_requests.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl LlmProvider for HarnessMockProvider {
        async fn list_models(&self) -> Result<Vec<ModelInfo>, GlossError> {
            Ok(Vec::new())
        }

        async fn chat(
            &self,
            _request: ChatRequest,
            ctx: LlmExecutionContext,
        ) -> Result<
            std::pin::Pin<Box<dyn futures::Stream<Item = Result<ChatToken, GlossError>> + Send>>,
            GlossError,
        > {
            self.active_requests.fetch_add(1, Ordering::SeqCst);
            match &self.mode {
                MockProviderMode::NeverStarts => {
                    ctx.cancellation.cancelled().await;
                    self.cancellation_observed.store(true, Ordering::SeqCst);
                    self.active_requests.fetch_sub(1, Ordering::SeqCst);
                    Err(provider_cancelled_error(
                        "ollama",
                        "mock_never_starts",
                        ctx.attempt_id.as_deref(),
                    ))
                }
                MockProviderMode::SlowStart { delay_ms } => {
                    tokio::select! {
                        _ = ctx.cancellation.cancelled() => {
                            self.cancellation_observed.store(true, Ordering::SeqCst);
                            self.active_requests.fetch_sub(1, Ordering::SeqCst);
                            Err(provider_cancelled_error("ollama", "mock_slow_start", ctx.attempt_id.as_deref()))
                        }
                        _ = sleep(Duration::from_millis(*delay_ms)) => {
                            self.active_requests.fetch_sub(1, Ordering::SeqCst);
                            Ok(Box::pin(stream::iter(vec![Ok(ChatToken { token: "started".into(), done: true })])))
                        }
                    }
                }
                _ => {
                    let state = MockStreamState {
                        mode: self.mode.clone(),
                        step: 0,
                        ctx,
                        active_requests: Arc::clone(&self.active_requests),
                        cancellation_observed: Arc::clone(&self.cancellation_observed),
                    };
                    let stream = stream::unfold(Some(state), |state| async move {
                        let mut state = state?;
                        if state.ctx.is_cancelled() {
                            state.cancellation_observed.store(true, Ordering::SeqCst);
                            return Some((
                                Err(provider_cancelled_error(
                                    "ollama",
                                    "mock_stream_cancelled",
                                    state.ctx.attempt_id.as_deref(),
                                )),
                                None,
                            ));
                        }
                        let item = match &state.mode {
                            MockProviderMode::SlowFirstToken { delay_ms } if state.step == 0 => {
                                sleep(Duration::from_millis(*delay_ms)).await;
                                Ok(ChatToken {
                                    token: "slow-first".into(),
                                    done: false,
                                })
                            }
                            MockProviderMode::IdleAfterChunks { chunks, idle_ms } => {
                                if let Some(chunk) = chunks.get(state.step) {
                                    Ok(ChatToken {
                                        token: (*chunk).to_string(),
                                        done: false,
                                    })
                                } else {
                                    sleep(Duration::from_millis(*idle_ms)).await;
                                    Ok(ChatToken {
                                        token: String::new(),
                                        done: false,
                                    })
                                }
                            }
                            MockProviderMode::LateChunksAfterCancel => Ok(ChatToken {
                                token: "before-cancel".into(),
                                done: false,
                            }),
                            MockProviderMode::IncompleteEof => {
                                if state.step == 0 {
                                    Ok(ChatToken {
                                        token: "partial".into(),
                                        done: false,
                                    })
                                } else {
                                    return None;
                                }
                            }
                            MockProviderMode::DoneNoContent => Ok(ChatToken {
                                token: String::new(),
                                done: true,
                            }),
                            MockProviderMode::Normal { chunks } => {
                                if let Some(chunk) = chunks.get(state.step) {
                                    Ok(ChatToken {
                                        token: (*chunk).to_string(),
                                        done: false,
                                    })
                                } else {
                                    Ok(ChatToken {
                                        token: String::new(),
                                        done: true,
                                    })
                                }
                            }
                            _ => Ok(ChatToken {
                                token: String::new(),
                                done: true,
                            }),
                        };
                        state.step += 1;
                        Some((item, Some(state)))
                    });
                    Ok(Box::pin(stream))
                }
            }
        }

        async fn health_check(&self) -> Result<bool, GlossError> {
            Ok(true)
        }

        fn provider_type(&self) -> ProviderType {
            ProviderType::Ollama
        }
    }

    fn mock_chat_request() -> ChatRequest {
        ChatRequest {
            model: "mock-model".to_string(),
            system_prompt: None,
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "hello".to_string(),
                images: None,
            }],
            max_tokens: 32,
            temperature: 0.0,
            top_p: None,
            top_k: None,
            min_p: None,
            repeat_penalty: None,
            stream: true,
            num_ctx: None,
        }
    }

    #[tokio::test]
    async fn mock_provider_never_starts_observes_cancellation_and_decrements_active() {
        let provider = HarnessMockProvider::new(MockProviderMode::NeverStarts);
        let cancellation = tokio_util::sync::CancellationToken::new();
        let ctx = LlmExecutionContext::default_with_token(cancellation.clone())
            .with_attempt_id("attempt-never-starts");

        let task = tokio::spawn({
            let provider = HarnessMockProvider {
                mode: provider.mode.clone(),
                active_requests: Arc::clone(&provider.active_requests),
                cancellation_observed: Arc::clone(&provider.cancellation_observed),
            };
            async move { provider.chat(mock_chat_request(), ctx).await }
        });
        while provider.active_requests.load(Ordering::SeqCst) == 0 {
            sleep(Duration::from_millis(1)).await;
        }
        cancellation.cancel();
        let error = match task.await.unwrap() {
            Ok(_) => panic!("cancelled start must fail"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("provider request cancelled"));
        assert!(provider.cancellation_observed.load(Ordering::SeqCst));
        assert_eq!(provider.active_requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn mock_provider_exercises_stream_shapes() {
        let cases = [
            (
                MockProviderMode::SlowStart { delay_ms: 1 },
                vec!["started"],
                true,
            ),
            (
                MockProviderMode::SlowFirstToken { delay_ms: 1 },
                vec!["slow-first", ""],
                true,
            ),
            (
                MockProviderMode::IdleAfterChunks {
                    chunks: vec!["a", "b"],
                    idle_ms: 1,
                },
                vec!["a", "b", ""],
                false,
            ),
            (MockProviderMode::IncompleteEof, vec!["partial"], false),
            (MockProviderMode::DoneNoContent, vec![""], true),
            (
                MockProviderMode::Normal {
                    chunks: vec!["hello", " world"],
                },
                vec!["hello", " world", ""],
                true,
            ),
        ];

        for (mode, expected_tokens, expected_terminal_done) in cases {
            let provider = HarnessMockProvider::new(mode);
            let ctx = LlmExecutionContext::uncancellable();
            let mut stream = provider.chat(mock_chat_request(), ctx).await.unwrap();
            let mut tokens = Vec::new();
            let mut done = false;
            while let Some(item) = stream.next().await {
                let token = item.unwrap();
                done = token.done;
                tokens.push(token.token);
                if tokens.len() >= expected_tokens.len() {
                    break;
                }
            }
            drop(stream);

            assert_eq!(tokens, expected_tokens);
            assert_eq!(done, expected_terminal_done);
            assert_eq!(provider.active_requests.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn mock_provider_late_chunks_after_cancel_surface_cancelled_error() {
        let provider = HarnessMockProvider::new(MockProviderMode::LateChunksAfterCancel);
        let cancellation = tokio_util::sync::CancellationToken::new();
        let ctx = LlmExecutionContext::default_with_token(cancellation.clone())
            .with_attempt_id("attempt-late-cancel");
        let mut stream = provider.chat(mock_chat_request(), ctx).await.unwrap();
        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first.token, "before-cancel");

        cancellation.cancel();
        let late = stream
            .next()
            .await
            .expect("mock should report attempted post-cancel chunk")
            .expect_err("post-cancel provider yield must become cancellation");

        assert!(late.to_string().contains("provider request cancelled"));
        drop(stream);
        assert!(provider.cancellation_observed.load(Ordering::SeqCst));
        assert_eq!(provider.active_requests.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn network_scope_policy_allows_loopback_local_providers() {
        let receipt = validate_provider_base_url(
            ProviderType::Ollama,
            "http://localhost:11434/",
            false,
            false,
        )
        .unwrap();
        assert_eq!(receipt.egress_class, "local_loopback");
        assert_eq!(receipt.base_url, "http://localhost:11434");
        assert!(!receipt.lan_opt_in_applied);
        assert!(validate_provider_base_url(
            ProviderType::LlamaCpp,
            "http://127.0.0.1:8080/v1",
            false,
            false
        )
        .is_ok());
    }

    #[test]
    fn network_scope_policy_rejects_lan_default() {
        // LAN IPs rejected by default (allow_lan = false)
        assert!(validate_provider_base_url(
            ProviderType::Ollama,
            "http://192.168.1.7:11434",
            false,
            false
        )
        .is_err());
        assert!(validate_provider_base_url(
            ProviderType::Ollama,
            "http://10.0.0.5:11434",
            false,
            false
        )
        .is_err());
        assert!(validate_provider_base_url(
            ProviderType::Ollama,
            "http://172.16.0.1:11434",
            false,
            false
        )
        .is_err());
        assert!(validate_provider_base_url(
            ProviderType::LlamaCpp,
            "http://192.168.1.100:8080/v1",
            false,
            false
        )
        .is_err());
    }

    #[test]
    fn network_scope_policy_allows_lan_with_opt_in() {
        // LAN IPs accepted when allow_lan = true
        let receipt = validate_provider_base_url(
            ProviderType::Ollama,
            "http://192.168.1.7:11434",
            true,
            false,
        )
        .unwrap();
        assert_eq!(receipt.egress_class, "local_lan");
        assert!(receipt.lan_opt_in_applied);

        let receipt2 =
            validate_provider_base_url(ProviderType::Ollama, "http://10.0.0.5:11434", true, false)
                .unwrap();
        assert_eq!(receipt2.egress_class, "local_lan");
        assert!(receipt2.lan_opt_in_applied);

        let receipt3 = validate_provider_base_url(
            ProviderType::LlamaCpp,
            "http://172.16.5.20:8080/v1",
            true,
            false,
        )
        .unwrap();
        assert_eq!(receipt3.egress_class, "local_lan");
        assert!(receipt3.lan_opt_in_applied);
    }

    #[test]
    fn network_scope_policy_rejects_public_ips_even_with_opt_in() {
        // Public IPs always rejected, even with allow_lan = true
        assert!(validate_provider_base_url(
            ProviderType::Ollama,
            "http://203.0.113.5:11434",
            true,
            false
        )
        .is_err());
        assert!(validate_provider_base_url(
            ProviderType::Ollama,
            "http://8.8.8.8:11434",
            true,
            false
        )
        .is_err());
    }

    #[test]
    fn network_scope_policy_rejects_credentials_query_fragment() {
        // Credentials in URLs always rejected
        assert!(validate_provider_base_url(
            ProviderType::OpenAI,
            "https://token@example.com/v1",
            false,
            false
        )
        .is_err());
        // Query strings always rejected
        assert!(validate_provider_base_url(
            ProviderType::Anthropic,
            "https://api.anthropic.com/v1?key=***\n",
            false,
            false
        )
        .is_err());
        // Fragment always rejected
        assert!(validate_provider_base_url(
            ProviderType::Ollama,
            "http://localhost:11434/#fragment",
            false,
            false
        )
        .is_err());
        // Credentials in URL even with LAN opt-in
        assert!(validate_provider_base_url(
            ProviderType::Ollama,
            "http://user:pass@192.168.1.7:11434",
            true,
            false
        )
        .is_err());
        // Query string even with LAN opt-in
        assert!(validate_provider_base_url(
            ProviderType::Ollama,
            "http://192.168.1.7:11434?token=abc",
            true,
            false
        )
        .is_err());
        // Fragment even with LAN opt-in
        assert!(validate_provider_base_url(
            ProviderType::Ollama,
            "http://192.168.1.7:11434#section",
            true,
            false
        )
        .is_err());
    }

    #[test]
    fn network_scope_policy_restricts_cloud_defaults() {
        assert!(validate_provider_base_url(
            ProviderType::OpenAI,
            "https://api.openai.com/v1",
            false,
            false
        )
        .is_ok());
        assert!(validate_provider_base_url(
            ProviderType::OpenAI,
            "https://openai-compatible.example.test/v1",
            false,
            false
        )
        .is_err());
        assert!(validate_provider_base_url(
            ProviderType::Anthropic,
            "https://api.anthropic.com/v1",
            false,
            false
        )
        .is_ok());
    }

    #[test]
    fn network_scope_policy_allows_custom_cloud_endpoints_when_opted_in() {
        let openai = validate_provider_base_url(
            ProviderType::OpenAI,
            "https://example.openai-compatible.azure.com/openai/deployments/gpt-4o",
            false,
            true,
        )
        .unwrap();
        assert_eq!(openai.egress_class, "custom_cloud");
        assert!(!openai.cloud_opt_in_required);

        let anthropic = validate_provider_base_url(
            ProviderType::Anthropic,
            "https://custom.anthropic.endpoint/v1",
            false,
            true,
        )
        .unwrap();
        assert_eq!(anthropic.egress_class, "custom_cloud");
        assert!(!anthropic.cloud_opt_in_required);
    }

    #[test]
    fn network_scope_policy_rejects_http_for_custom_cloud_opt_in() {
        assert!(validate_provider_base_url(
            ProviderType::OpenAI,
            "http://api.openai.com/v1",
            false,
            true
        )
        .is_err());
        assert!(validate_provider_base_url(
            ProviderType::Anthropic,
            "http://custom.anthropic.endpoint/v1",
            false,
            true
        )
        .is_err());
    }

    #[test]
    fn lan_opt_in_not_applied_for_loopback() {
        let receipt = validate_provider_base_url(
            ProviderType::Ollama,
            "http://localhost:11434/",
            true,
            false,
        )
        .unwrap();
        // Even with allow_lan=true, loopback should be local_loopback, not local_lan
        assert_eq!(receipt.egress_class, "local_loopback");
        assert!(!receipt.lan_opt_in_applied);
    }

    #[test]
    fn is_rfc1918_host_detection() {
        // 10.0.0.0/8
        assert!(is_rfc1918_host("10.0.0.1"));
        assert!(is_rfc1918_host("10.255.255.255"));
        // 172.16.0.0/12
        assert!(is_rfc1918_host("172.16.0.1"));
        assert!(is_rfc1918_host("172.31.255.255"));
        assert!(!is_rfc1918_host("172.15.0.1"));
        assert!(!is_rfc1918_host("172.32.0.1"));
        // 192.168.0.0/16
        assert!(is_rfc1918_host("192.168.0.1"));
        assert!(is_rfc1918_host("192.168.255.255"));
        // Not RFC1918
        assert!(!is_rfc1918_host("8.8.8.8"));
        assert!(!is_rfc1918_host("203.0.113.1"));
        // Loopback is not RFC1918
        assert!(!is_rfc1918_host("127.0.0.1"));
    }

    #[test]
    fn provider_error_body_redacts_bearer_value_after_authorization_header() {
        let raw_value = "abcdefghijklmnopqrstuvwxyzABCDEF0123456789";
        let sanitized = sanitize_provider_error_body(&format!(
            "upstream error Authorization: Bearer {raw_value} done"
        ));
        assert!(!sanitized.contains(raw_value));
        assert!(!sanitized.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(sanitized.contains("[redacted]"));
    }

    #[test]
    fn provider_error_body_redacts_secrets_and_bounds_output() {
        let sanitized = sanitize_provider_error_body(
            "bad Authorization: Bearer *** token secret api_key=abc prompt text",
        );
        assert!(sanitized.contains("[redacted]"));
        assert!(!sanitized.contains("sk-test"));
        assert!(!sanitized.contains("api_key=abc"));
        assert!(sanitized.len() <= 243);
    }
}
