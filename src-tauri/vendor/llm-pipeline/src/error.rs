use std::time::Duration;
use thiserror::Error;

/// Errors produced by the pipeline and its components.
#[derive(Error, Debug)]
pub enum PipelineError {
    /// Low-level HTTP transport failure (connection refused, timeout, etc.).
    #[error("HTTP request failed: {0}")]
    Request(#[from] reqwest::Error),

    /// JSON parsing failed at the serde level.
    #[error("JSON parsing failed: {0}")]
    Json(#[from] serde_json::Error),

    /// Structured output parsing failed in `llm-output-parser`.
    #[error("Output parsing failed for {strategy}: {source}")]
    Parse {
        /// Parser surface being used (`json`, `string_list`, etc.).
        strategy: &'static str,
        /// Underlying parser error with preserved context.
        #[source]
        source: llm_output_parser::ParseError,
    },

    /// A pipeline stage failed with a descriptive message.
    #[error("Stage '{stage}' failed: {message}")]
    StageFailed { stage: String, message: String },

    /// The pipeline or payload was cancelled via the cancellation flag.
    #[error("Pipeline was cancelled")]
    Cancelled,

    /// Invalid configuration detected at build time.
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    /// HTTP error with status code, response body, and optional Retry-After hint.
    ///
    /// Returned by [`Backend`](crate::backend::Backend) implementations when
    /// the provider returns a non-success status code. The `retry_after` field
    /// is populated from the `Retry-After` response header when present.
    #[error("HTTP {status}: {body}")]
    HttpError {
        /// HTTP status code (e.g. 429, 500, 503).
        status: u16,
        /// Response body text.
        body: String,
        /// Parsed `Retry-After` header value, if present.
        retry_after: Option<Duration>,
    },

    /// LLM response exceeded the configured size limit.
    #[error("Response too large: {size} bytes exceeds limit of {limit} bytes")]
    ResponseTooLarge {
        /// Actual response size in bytes.
        size: usize,
        /// Configured limit in bytes.
        limit: usize,
    },

    /// No tokens received within the configured stream idle timeout.
    #[error("Stream idle: no tokens received for {idle_ms}ms (limit: {limit_ms}ms)")]
    StreamIdle {
        /// Duration of silence in milliseconds.
        idle_ms: u64,
        /// Configured idle limit in milliseconds.
        limit_ms: u64,
    },

    /// Operation exceeded the configured wall-clock timeout.
    #[error("Timeout: operation took {elapsed_ms}ms (limit: {limit_ms}ms)")]
    Timeout {
        /// Elapsed time in milliseconds.
        elapsed_ms: u64,
        /// Configured timeout in milliseconds.
        limit_ms: u64,
    },

    /// Catch-all for other errors.
    #[error("{0}")]
    Other(String),
}

impl PipelineError {
    /// Returns a stable string discriminant for programmatic matching.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Request(_) => "request",
            Self::Json(_) => "json",
            Self::Parse { .. } => "parse",
            Self::StageFailed { .. } => "stage_failed",
            Self::Cancelled => "cancelled",
            Self::InvalidConfig(_) => "invalid_config",
            Self::HttpError { .. } => "http_error",
            Self::ResponseTooLarge { .. } => "response_too_large",
            Self::StreamIdle { .. } => "stream_idle",
            Self::Timeout { .. } => "timeout",
            Self::Other(_) => "other",
        }
    }
}

impl From<anyhow::Error> for PipelineError {
    fn from(err: anyhow::Error) -> Self {
        PipelineError::Other(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, PipelineError>;
