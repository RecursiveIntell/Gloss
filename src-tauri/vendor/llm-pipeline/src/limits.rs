//! Resource limits for pipeline operations.
//!
//! [`PipelineLimits`] enforces bounded resource usage for LLM calls.
//! All limits have safe defaults and produce typed errors when exceeded.

use std::time::Duration;

/// Resource limits for pipeline operations.
///
/// Enforced at the public API boundary. Exceeding any limit returns a
/// typed [`PipelineError`](crate::PipelineError) variant, never a panic.
///
/// # Defaults
///
/// Tuned for a GTX 1070 / 8 GB VRAM laptop server:
///
/// | Limit | Default | Rationale |
/// |-------|---------|-----------|
/// | `max_response_bytes` | 2 MB | Prevents unbounded memory from runaway generation |
/// | `request_timeout` | 120 s | Long enough for complex prompts, short enough to detect hangs |
/// | `stream_idle_timeout` | 30 s | Detects stalled streams without killing slow generation |
///
/// # Example
///
/// ```
/// use llm_pipeline::PipelineLimits;
/// use std::time::Duration;
///
/// let limits = PipelineLimits::default();
/// assert_eq!(limits.max_response_bytes, 2_097_152);
///
/// let strict = PipelineLimits {
///     max_response_bytes: 1024 * 512,
///     request_timeout: Duration::from_secs(30),
///     stream_idle_timeout: Duration::from_secs(10),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct PipelineLimits {
    /// Maximum size of an LLM response in bytes. Responses exceeding this
    /// limit produce [`PipelineError::ResponseTooLarge`](crate::PipelineError::ResponseTooLarge).
    pub max_response_bytes: usize,

    /// Total wall-clock timeout for a single LLM request (including retries).
    /// Exceeding this produces [`PipelineError::Timeout`](crate::PipelineError::Timeout).
    pub request_timeout: Duration,

    /// Maximum time between consecutive tokens during streaming.
    /// Exceeding this produces [`PipelineError::StreamIdle`](crate::PipelineError::StreamIdle).
    pub stream_idle_timeout: Duration,
}

impl Default for PipelineLimits {
    fn default() -> Self {
        Self {
            max_response_bytes: 2 * 1024 * 1024, // 2 MB
            request_timeout: Duration::from_secs(120),
            stream_idle_timeout: Duration::from_secs(30),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_limits() {
        let limits = PipelineLimits::default();
        assert_eq!(limits.max_response_bytes, 2_097_152);
        assert_eq!(limits.request_timeout, Duration::from_secs(120));
        assert_eq!(limits.stream_idle_timeout, Duration::from_secs(30));
    }

    #[test]
    fn test_custom_limits() {
        let limits = PipelineLimits {
            max_response_bytes: 1024,
            request_timeout: Duration::from_secs(10),
            stream_idle_timeout: Duration::from_secs(5),
        };
        assert_eq!(limits.max_response_bytes, 1024);
    }
}
