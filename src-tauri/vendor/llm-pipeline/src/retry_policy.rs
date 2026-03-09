//! Named retry policy types for transport and semantic retries.
//!
//! These types make the distinction between transport-level retries (HTTP 429,
//! 5xx, connection failures) and semantic-level retries (parse failures,
//! validation errors) explicit in the API.

use crate::backend::BackoffConfig;
use crate::retry::{RetryConfig, ValidatorFn};
use std::time::Duration;

/// Transport-level retry policy for HTTP failures (429, 5xx, connection errors).
///
/// Wraps the same concepts as [`BackoffConfig`] with a more descriptive API.
/// Converts to/from `BackoffConfig` for use with the backend layer.
///
/// # Example
///
/// ```
/// use llm_pipeline::TransportRetryPolicy;
/// use std::time::Duration;
///
/// let policy = TransportRetryPolicy::default();
/// assert_eq!(policy.max_retries, 3);
///
/// let aggressive = TransportRetryPolicy {
///     max_retries: 5,
///     initial_delay: Duration::from_millis(500),
///     max_delay: Duration::from_secs(30),
///     jitter: true,
///     retryable_statuses: vec![429, 500, 502, 503, 504],
///     respect_retry_after: true,
/// };
/// ```
#[derive(Debug, Clone)]
pub struct TransportRetryPolicy {
    /// Maximum number of retry attempts. Hard-capped at 5.
    pub max_retries: u32,

    /// Delay before the first retry.
    pub initial_delay: Duration,

    /// Maximum delay between retries.
    pub max_delay: Duration,

    /// Whether to add random jitter to delays.
    pub jitter: bool,

    /// HTTP status codes that trigger a retry.
    pub retryable_statuses: Vec<u16>,

    /// Whether to respect `Retry-After` headers from the server.
    pub respect_retry_after: bool,
}

impl Default for TransportRetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            jitter: true,
            retryable_statuses: vec![429, 500, 502, 503, 504],
            respect_retry_after: true,
        }
    }
}

impl TransportRetryPolicy {
    /// No transport retries.
    pub fn none() -> Self {
        Self {
            max_retries: 0,
            ..Default::default()
        }
    }

    /// Convert to a [`BackoffConfig`] for use with the backend layer.
    pub fn to_backoff_config(&self) -> BackoffConfig {
        BackoffConfig {
            max_retries: self.max_retries.min(5),
            initial_delay: self.initial_delay,
            multiplier: 2.0,
            max_delay: self.max_delay,
            jitter: if self.jitter {
                crate::backend::backoff::JitterStrategy::Full
            } else {
                crate::backend::backoff::JitterStrategy::None
            },
            retryable_statuses: self.retryable_statuses.clone(),
            respect_retry_after: self.respect_retry_after,
        }
    }
}

impl From<BackoffConfig> for TransportRetryPolicy {
    fn from(config: BackoffConfig) -> Self {
        Self {
            max_retries: config.max_retries,
            initial_delay: config.initial_delay,
            max_delay: config.max_delay,
            jitter: !matches!(config.jitter, crate::backend::backoff::JitterStrategy::None),
            retryable_statuses: config.retryable_statuses,
            respect_retry_after: config.respect_retry_after,
        }
    }
}

/// Semantic-level retry policy for parse failures and validation errors.
///
/// Wraps the same concepts as [`RetryConfig`] with a more descriptive API.
/// Converts to/from `RetryConfig` for use with the LLM call layer.
///
/// # Example
///
/// ```
/// use llm_pipeline::SemanticRetryPolicy;
///
/// let policy = SemanticRetryPolicy::default();
/// assert_eq!(policy.max_retries, 2);
///
/// let strict = SemanticRetryPolicy::new(3)
///     .requiring_keys(&["title", "year"]);
/// ```
#[derive(Clone)]
pub struct SemanticRetryPolicy {
    /// Maximum retry attempts on parse/validation failure. Hard-capped at 5.
    pub max_retries: u32,

    /// Lower temperature on each retry. Default: `true`.
    pub cool_down: bool,

    /// Optional semantic validator beyond the output strategy.
    pub validator: Option<ValidatorFn>,
}

impl Default for SemanticRetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            cool_down: true,
            validator: None,
        }
    }
}

impl SemanticRetryPolicy {
    /// Create a policy with the given max retries (capped at 5).
    pub fn new(max_retries: u32) -> Self {
        Self {
            max_retries: max_retries.min(5),
            ..Default::default()
        }
    }

    /// Add a semantic validator.
    pub fn with_validator(
        mut self,
        f: impl Fn(&str, &serde_json::Value) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.validator = Some(std::sync::Arc::new(f));
        self
    }

    /// Shorthand: validate that specific JSON keys exist and are non-null.
    pub fn requiring_keys(self, keys: &[&str]) -> Self {
        let keys: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
        self.with_validator(move |_raw, value| {
            for key in &keys {
                match value.get(key.as_str()) {
                    None => return Err(format!("missing required key: '{}'", key)),
                    Some(v) if v.is_null() => {
                        return Err(format!("required key '{}' is null", key))
                    }
                    _ => {}
                }
            }
            Ok(())
        })
    }

    /// Disable temperature cool-down.
    pub fn no_cool_down(mut self) -> Self {
        self.cool_down = false;
        self
    }

    /// Convert to a [`RetryConfig`] for use with the LLM call layer.
    pub fn to_retry_config(&self) -> RetryConfig {
        let mut config = RetryConfig::new(self.max_retries);
        config.cool_down = self.cool_down;
        config.validator = self.validator.clone();
        config
    }
}

impl From<RetryConfig> for SemanticRetryPolicy {
    fn from(config: RetryConfig) -> Self {
        Self {
            max_retries: config.max_retries,
            cool_down: config.cool_down,
            validator: config.validator,
        }
    }
}

impl std::fmt::Debug for SemanticRetryPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SemanticRetryPolicy")
            .field("max_retries", &self.max_retries)
            .field("cool_down", &self.cool_down)
            .field("has_validator", &self.validator.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transport_retry_default() {
        let policy = TransportRetryPolicy::default();
        assert_eq!(policy.max_retries, 3);
        assert!(policy.jitter);
        assert!(policy.retryable_statuses.contains(&429));
    }

    #[test]
    fn test_transport_retry_none() {
        let policy = TransportRetryPolicy::none();
        assert_eq!(policy.max_retries, 0);
    }

    #[test]
    fn test_transport_retry_to_backoff_config() {
        let policy = TransportRetryPolicy::default();
        let config = policy.to_backoff_config();
        assert_eq!(config.max_retries, 3);
        assert!(config.retryable_statuses.contains(&429));
    }

    #[test]
    fn test_transport_retry_roundtrip() {
        let original = BackoffConfig::standard();
        let policy: TransportRetryPolicy = original.clone().into();
        let back = policy.to_backoff_config();
        assert_eq!(original.max_retries, back.max_retries);
    }

    #[test]
    fn test_semantic_retry_default() {
        let policy = SemanticRetryPolicy::default();
        assert_eq!(policy.max_retries, 2);
        assert!(policy.cool_down);
        assert!(policy.validator.is_none());
    }

    #[test]
    fn test_semantic_retry_capped_at_5() {
        let policy = SemanticRetryPolicy::new(10);
        assert_eq!(policy.max_retries, 5);
    }

    #[test]
    fn test_semantic_retry_requiring_keys() {
        let policy = SemanticRetryPolicy::new(2).requiring_keys(&["title", "year"]);
        assert!(policy.validator.is_some());

        let good = serde_json::json!({"title": "Matrix", "year": 1999});
        assert!(policy.validator.as_ref().unwrap()("", &good).is_ok());

        let bad = serde_json::json!({"title": "Matrix"});
        assert!(policy.validator.as_ref().unwrap()("", &bad).is_err());
    }

    #[test]
    fn test_semantic_retry_to_retry_config() {
        let policy = SemanticRetryPolicy::new(3).no_cool_down();
        let config = policy.to_retry_config();
        assert_eq!(config.max_retries, 3);
        assert!(!config.cool_down);
    }

    #[test]
    fn test_semantic_retry_from_retry_config() {
        let config = RetryConfig::new(2);
        let policy: SemanticRetryPolicy = config.into();
        assert_eq!(policy.max_retries, 2);
        assert!(policy.cool_down);
    }
}
