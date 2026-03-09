//! Execution context shared across payload invocations.
//!
//! [`ExecCtx`] carries the HTTP client, LLM backend, endpoint, template variables,
//! cancellation handle, and optional event handler. It is designed to be
//! constructed once and shared across all payloads in a chain or graph.

#[cfg(feature = "openai")]
use crate::backend::OpenAiBackend;
use crate::backend::{Backend, BackoffConfig, OllamaBackend};
use crate::events::EventHandler;
use crate::limits::PipelineLimits;
#[allow(deprecated)]
use crate::trace::TraceId;
use reqwest::Client;
use stack_ids::TraceCtx;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

/// Shared execution context for payload invocations.
///
/// Carries everything a payload needs from the runtime environment
/// without coupling to any specific orchestrator (Pipeline, LangGraph, etc.).
///
/// # Example
///
/// ```
/// use llm_pipeline::ExecCtx;
///
/// let ctx = ExecCtx::builder("http://localhost:11434")
///     .var("domain", "science")
///     .var("audience", "researchers")
///     .build();
/// ```
#[allow(deprecated)]
pub struct ExecCtx {
    /// HTTP client (cheap to clone -- uses `Arc` internally).
    pub client: Client,
    /// Base URL for the LLM provider (e.g. `http://localhost:11434`).
    pub base_url: String,
    /// LLM backend. Default: [`OllamaBackend`].
    pub backend: Arc<dyn Backend>,
    /// Transport retry configuration. Default: [`BackoffConfig::none()`].
    pub backoff: BackoffConfig,
    /// Template variables substituted into prompt `{key}` placeholders.
    pub vars: HashMap<String, String>,
    /// Optional cancellation flag; payloads should check before starting.
    pub cancellation: Option<Arc<AtomicBool>>,
    /// Optional event handler for streaming tokens and lifecycle events.
    pub event_handler: Option<Arc<dyn EventHandler>>,
    /// Phase status: compatibility / migration-only
    ///
    /// Legacy trace ID for correlating this context's operations across crates.
    /// Auto-generated if not provided. Use [`trace_ctx`](Self::trace_ctx) for
    /// the canonical trace form on the normal path.
    ///
    /// **Removal condition**: removed when all callers migrate to `TraceCtx`.
    #[deprecated(note = "Use trace_ctx instead. Will be removed when all callers migrate to TraceCtx.")]
    pub trace_id: TraceId,
    /// Canonical trace context from `stack_ids`.
    ///
    /// This is the normal-path trace form. It supports parent span tracking,
    /// bounded baggage, and W3C traceparent serialization. Automatically
    /// generated from `trace_id` if not explicitly set.
    pub trace_ctx: TraceCtx,
    /// Resource limits for pipeline operations.
    pub limits: PipelineLimits,
}

#[allow(deprecated)]
impl ExecCtx {
    /// Create a new builder.
    pub fn builder(base_url: impl Into<String>) -> ExecCtxBuilder {
        ExecCtxBuilder {
            client: None,
            base_url: base_url.into(),
            backend: None,
            backoff: None,
            vars: HashMap::new(),
            cancellation: None,
            event_handler: None,
            timeout: None,
            trace_id: None,
            trace_ctx: None,
            limits: None,
        }
    }

    /// Check whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancellation
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
    }

    /// Return an error if cancellation has been requested.
    pub fn check_cancelled(&self) -> crate::error::Result<()> {
        if self.is_cancelled() {
            return Err(crate::PipelineError::Cancelled);
        }
        Ok(())
    }

    /// Get a reference to the cancellation AtomicBool, if set.
    pub fn cancel_flag(&self) -> Option<&AtomicBool> {
        self.cancellation.as_deref()
    }
}

#[allow(deprecated)]
impl std::fmt::Debug for ExecCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecCtx")
            .field("base_url", &self.base_url)
            .field("backend", &self.backend.name())
            .field("backoff", &self.backoff)
            .field("vars_count", &self.vars.len())
            .field("has_cancellation", &self.cancellation.is_some())
            .field("has_event_handler", &self.event_handler.is_some())
            .field("trace_id", &self.trace_id)
            .field("trace_ctx", &self.trace_ctx)
            .field("limits", &self.limits)
            .finish()
    }
}

/// Builder for [`ExecCtx`].
pub struct ExecCtxBuilder {
    client: Option<Client>,
    base_url: String,
    backend: Option<Arc<dyn Backend>>,
    backoff: Option<BackoffConfig>,
    vars: HashMap<String, String>,
    cancellation: Option<Arc<AtomicBool>>,
    event_handler: Option<Arc<dyn EventHandler>>,
    timeout: Option<Duration>,
    trace_id: Option<TraceId>,
    trace_ctx: Option<TraceCtx>,
    limits: Option<PipelineLimits>,
}

#[allow(deprecated)]
impl ExecCtxBuilder {
    /// Set the HTTP client. If not set, a default client is created.
    pub fn client(mut self, client: Client) -> Self {
        self.client = Some(client);
        self
    }

    /// Set the LLM backend. Default: [`OllamaBackend`].
    pub fn backend(mut self, backend: Arc<dyn Backend>) -> Self {
        self.backend = Some(backend);
        self
    }

    /// Use the OpenAI-compatible backend without authentication.
    ///
    /// Sets the backend to [`OpenAiBackend`] with no API key. If the provider
    /// requires authentication, use [`openai_with_key`](Self::openai_with_key) instead.
    #[cfg(feature = "openai")]
    pub fn openai(mut self) -> Self {
        self.backend = Some(Arc::new(OpenAiBackend::new()));
        self
    }

    /// Use the OpenAI-compatible backend with API key authentication.
    ///
    /// Sets the backend to [`OpenAiBackend`] with the given API key sent as
    /// `Authorization: Bearer {key}`.
    #[cfg(feature = "openai")]
    pub fn openai_with_key(mut self, api_key: impl Into<String>) -> Self {
        self.backend = Some(Arc::new(OpenAiBackend::new().with_api_key(api_key)));
        self
    }

    /// Set the transport retry configuration. Default: [`BackoffConfig::none()`].
    pub fn backoff(mut self, config: BackoffConfig) -> Self {
        self.backoff = Some(config);
        self
    }

    /// Set all template variables at once.
    pub fn vars(mut self, vars: HashMap<String, String>) -> Self {
        self.vars = vars;
        self
    }

    /// Insert a single template variable.
    pub fn var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.vars.insert(key.into(), value.into());
        self
    }

    /// Set the cancellation flag.
    pub fn cancellation(mut self, cancel: Option<Arc<AtomicBool>>) -> Self {
        self.cancellation = cancel;
        self
    }

    /// Set the event handler.
    pub fn event_handler(mut self, handler: Arc<dyn EventHandler>) -> Self {
        self.event_handler = Some(handler);
        self
    }

    /// Set the request timeout. Default: 60 seconds.
    ///
    /// If no custom `Client` is provided, the built client will use this timeout.
    /// If a custom `Client` is provided via `.client()`, this setting is ignored
    /// (the custom client's own timeout applies).
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Phase status: compatibility / migration-only
    ///
    /// Set the legacy trace ID for correlating operations. If not set, a random
    /// UUID v4 is generated automatically.
    ///
    /// Prefer [`with_trace_ctx`](Self::with_trace_ctx) for new code.
    ///
    /// **Removal condition**: removed when all callers migrate to `TraceCtx`.
    #[deprecated(since = "0.6.0", note = "Use with_trace_ctx() instead. This method will be removed in v1.0.")]
    pub fn with_trace_id(mut self, trace_id: TraceId) -> Self {
        self.trace_id = Some(trace_id);
        self
    }

    /// Set the canonical trace context from `stack_ids::TraceCtx`.
    ///
    /// This is the normal-path trace form. When set, the legacy `trace_id` field
    /// is derived from `trace_ctx.trace_id` for backward compatibility.
    pub fn with_trace_ctx(mut self, trace_ctx: TraceCtx) -> Self {
        self.trace_ctx = Some(trace_ctx);
        self
    }

    /// Set resource limits. If not set, [`PipelineLimits::default()`] is used.
    pub fn with_limits(mut self, limits: PipelineLimits) -> Self {
        self.limits = Some(limits);
        self
    }

    /// Build the execution context.
    ///
    /// **Preferred**: use [`with_trace_ctx()`](Self::with_trace_ctx) to set trace identity.
    /// The legacy `with_trace_id()` method is deprecated.
    ///
    /// Resolution order for trace identity:
    /// 1. **(Canonical)** If `trace_ctx` was set explicitly via `with_trace_ctx()`, use it. Derive legacy `trace_id` from it at the compatibility boundary.
    /// 2. **(Legacy/compat)** If only `trace_id` was set via deprecated `with_trace_id()`, derive `trace_ctx` from it.
    /// 3. **(Default)** If neither was set, generate a fresh `TraceCtx` and derive legacy `trace_id`.
    ///
    /// The legacy `trace_id` is always derived — never independently generated — when
    /// `trace_ctx` is present. This ensures a single source of truth for trace identity.
    pub fn build(self) -> ExecCtx {
        let limits = self.limits.unwrap_or_default();
        let timeout = self.timeout.unwrap_or(limits.request_timeout);
        let client = self.client.unwrap_or_else(|| {
            Client::builder()
                .timeout(timeout)
                .build()
                .expect("Failed to build HTTP client")
        });

        let (trace_id, trace_ctx) = match (self.trace_ctx, self.trace_id) {
            // Canonical path: TraceCtx was explicitly set
            (Some(ctx), _) => {
                let legacy = TraceId::from_trace_ctx(&ctx);
                (legacy, ctx)
            }
            // Compat path: only legacy TraceId was set
            (None, Some(id)) => {
                let ctx = id.to_trace_ctx();
                (id, ctx)
            }
            // Default: generate fresh TraceCtx
            (None, None) => {
                let ctx = TraceCtx::generate();
                let legacy = TraceId::from_trace_ctx(&ctx);
                (legacy, ctx)
            }
        };

        ExecCtx {
            client,
            base_url: normalize_base_url(&self.base_url),
            backend: self.backend.unwrap_or_else(|| Arc::new(OllamaBackend)),
            backoff: self.backoff.unwrap_or_else(BackoffConfig::none),
            vars: self.vars,
            cancellation: self.cancellation,
            event_handler: self.event_handler,
            trace_id,
            trace_ctx,
            limits,
        }
    }
}

/// Strip known provider path suffixes from a base URL.
/// This prevents double-pathing when backends append their own paths.
/// e.g., "https://api.openai.com/v1" -> "https://api.openai.com"
/// e.g., "http://localhost:11434/api" -> "http://localhost:11434"
fn normalize_base_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    // Strip known suffixes (order matters — longest first)
    for suffix in &[
        "/v1/chat/completions",
        "/v1/chat",
        "/v1",
        "/api/generate",
        "/api/chat",
        "/api",
    ] {
        if let Some(stripped) = trimmed.strip_suffix(suffix) {
            return stripped.to_string();
        }
    }
    trimmed.to_string()
}

#[allow(deprecated)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_base_url_strips_v1() {
        assert_eq!(
            normalize_base_url("https://api.openai.com/v1"),
            "https://api.openai.com"
        );
        assert_eq!(
            normalize_base_url("https://api.openai.com/v1/"),
            "https://api.openai.com"
        );
    }

    #[test]
    fn test_normalize_base_url_strips_api() {
        assert_eq!(
            normalize_base_url("http://localhost:11434/api"),
            "http://localhost:11434"
        );
        assert_eq!(
            normalize_base_url("http://localhost:11434/api/"),
            "http://localhost:11434"
        );
    }

    #[test]
    fn test_normalize_base_url_preserves_clean() {
        assert_eq!(
            normalize_base_url("http://localhost:11434"),
            "http://localhost:11434"
        );
        assert_eq!(
            normalize_base_url("https://api.openai.com"),
            "https://api.openai.com"
        );
    }

    #[test]
    fn test_normalize_base_url_strips_full_path() {
        assert_eq!(
            normalize_base_url("https://api.openai.com/v1/chat/completions"),
            "https://api.openai.com"
        );
    }

    #[test]
    fn test_normalize_base_url_trailing_slash() {
        assert_eq!(
            normalize_base_url("http://localhost:11434/"),
            "http://localhost:11434"
        );
    }

    #[test]
    fn test_default_timeout_applied() {
        // Verify the builder accepts the timeout method and compiles.
        let _ctx = ExecCtx::builder("http://localhost:11434")
            .timeout(Duration::from_secs(120))
            .build();
        // Smoke test: builds without panic
    }

    #[test]
    fn test_trace_ctx_generated_by_default() {
        let ctx = ExecCtx::builder("http://localhost:11434").build();
        // Both trace forms should be populated
        assert!(!ctx.trace_id.as_str().is_empty());
        assert!(!ctx.trace_ctx.trace_id.is_empty());
    }

    #[test]
    fn test_trace_ctx_explicit_sets_legacy() {
        let trace = TraceCtx::from_trace_id("0af7651916cd43dd8448eb211c80319c");
        let ctx = ExecCtx::builder("http://localhost:11434")
            .with_trace_ctx(trace.clone())
            .build();
        assert_eq!(ctx.trace_ctx.trace_id, "0af7651916cd43dd8448eb211c80319c");
        assert_eq!(ctx.trace_id.as_str(), "0af7651916cd43dd8448eb211c80319c");
    }

    #[test]
    fn test_legacy_trace_id_derives_trace_ctx() {
        let id = TraceId::from_string("my-legacy-trace");
        let ctx = ExecCtx::builder("http://localhost:11434")
            .with_trace_id(id)
            .build();
        assert_eq!(ctx.trace_id.as_str(), "my-legacy-trace");
        assert_eq!(ctx.trace_ctx.trace_id, "my-legacy-trace");
    }

    #[test]
    fn test_trace_ctx_takes_priority_over_trace_id() {
        let trace = TraceCtx::from_trace_id("canonical-trace");
        let id = TraceId::from_string("legacy-trace");
        let ctx = ExecCtx::builder("http://localhost:11434")
            .with_trace_id(id)
            .with_trace_ctx(trace)
            .build();
        // TraceCtx wins
        assert_eq!(ctx.trace_ctx.trace_id, "canonical-trace");
        assert_eq!(ctx.trace_id.as_str(), "canonical-trace");
    }
}
