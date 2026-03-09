//! Transport-level retry with exponential backoff and jitter.
//!
//! [`BackoffConfig`] controls how transient HTTP errors (429, 5xx) are retried
//! with increasing delays. For local Ollama, use [`BackoffConfig::none()`].
//! For cloud APIs (OpenAI, Groq, Together), use [`BackoffConfig::standard()`]
//! or tune to your rate limit tier.

use std::time::Duration;

/// Configuration for transport-level retry with exponential backoff and jitter.
///
/// Handles transient HTTP errors (429 rate limit, 500/502/503 server errors,
/// connection timeouts) by retrying with increasing delays.
///
/// # Example
///
/// ```
/// use llm_pipeline::backend::BackoffConfig;
///
/// // No retry (default for backward compat)
/// let none = BackoffConfig::none();
/// assert_eq!(none.max_retries, 0);
///
/// // Standard cloud API settings
/// let standard = BackoffConfig::standard();
/// assert_eq!(standard.max_retries, 3);
/// ```
#[derive(Debug, Clone)]
pub struct BackoffConfig {
    /// Maximum number of transport retries. Default: 0 (no retry).
    pub max_retries: u32,

    /// Initial delay before first retry. Default: 1 second.
    pub initial_delay: Duration,

    /// Multiplier applied to delay after each retry. Default: 2.0.
    /// Delay grows: initial, initial * multiplier, initial * multiplier^2, ...
    pub multiplier: f64,

    /// Maximum delay between retries. Default: 60 seconds.
    /// Prevents exponential blowup on sustained outages.
    pub max_delay: Duration,

    /// Jitter strategy. Default: Full.
    pub jitter: JitterStrategy,

    /// HTTP status codes that trigger retry. Default: `[429, 500, 502, 503, 504]`.
    pub retryable_statuses: Vec<u16>,

    /// Whether to respect `Retry-After` headers from the provider.
    /// Default: `true`.
    pub respect_retry_after: bool,
}

/// Jitter strategy to prevent thundering herd on shared rate limits.
///
/// # Example
///
/// ```
/// use llm_pipeline::backend::backoff::JitterStrategy;
///
/// let jitter = JitterStrategy::Full;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitterStrategy {
    /// No jitter. Delay is exactly the calculated value.
    None,

    /// Full jitter: random value in `[0, calculated_delay]`.
    /// Recommended default (AWS best practice).
    Full,

    /// Equal jitter: `calculated_delay/2 + random in [0, calculated_delay/2]`.
    Equal,

    /// Decorrelated jitter: `random in [initial_delay, last_delay * 3]`.
    Decorrelated,
}

impl BackoffConfig {
    /// No transport retry. For local Ollama or when you handle errors yourself.
    ///
    /// This is the default to preserve backward compatibility.
    pub fn none() -> Self {
        Self {
            max_retries: 0,
            ..Self::standard()
        }
    }

    /// Sensible defaults for cloud APIs: 3 retries, 1s initial, 2x multiplier,
    /// 60s max, full jitter, respects Retry-After.
    pub fn standard() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_secs(1),
            multiplier: 2.0,
            max_delay: Duration::from_secs(60),
            jitter: JitterStrategy::Full,
            retryable_statuses: vec![429, 500, 502, 503, 504],
            respect_retry_after: true,
        }
    }

    /// Aggressive retry for high-throughput batch processing:
    /// 5 retries, 500ms initial, respects Retry-After.
    pub fn aggressive() -> Self {
        Self {
            max_retries: 5,
            initial_delay: Duration::from_millis(500),
            multiplier: 2.0,
            max_delay: Duration::from_secs(120),
            jitter: JitterStrategy::Full,
            retryable_statuses: vec![429, 500, 502, 503, 504],
            respect_retry_after: true,
        }
    }

    /// Conservative retry for interactive use (user is waiting):
    /// 2 retries, 500ms initial, 10s max.
    pub fn interactive() -> Self {
        Self {
            max_retries: 2,
            initial_delay: Duration::from_millis(500),
            multiplier: 1.5,
            max_delay: Duration::from_secs(10),
            jitter: JitterStrategy::Full,
            retryable_statuses: vec![429, 500, 502, 503, 504],
            respect_retry_after: true,
        }
    }

    /// Calculate the delay for attempt N (0-indexed).
    ///
    /// The base delay is `initial_delay * multiplier^attempt`, capped at
    /// `max_delay`. Jitter is then applied according to the configured strategy.
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let base = self.initial_delay.as_secs_f64() * self.multiplier.powi(attempt as i32);
        let capped = base.min(self.max_delay.as_secs_f64());

        let jittered = match self.jitter {
            JitterStrategy::None => capped,
            JitterStrategy::Full => fastrand::f64() * capped,
            JitterStrategy::Equal => capped / 2.0 + fastrand::f64() * (capped / 2.0),
            JitterStrategy::Decorrelated => {
                // Simplified: random in [0, capped]. Full decorrelated tracking
                // happens in the retry loop itself.
                fastrand::f64() * capped
            }
        };

        Duration::from_secs_f64(jittered)
    }
}

impl Default for BackoffConfig {
    fn default() -> Self {
        Self::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backoff_delay_exponential() {
        let config = BackoffConfig {
            max_retries: 5,
            initial_delay: Duration::from_secs(1),
            multiplier: 2.0,
            max_delay: Duration::from_secs(60),
            jitter: JitterStrategy::None,
            retryable_statuses: vec![429],
            respect_retry_after: false,
        };

        let d0 = config.delay_for_attempt(0);
        let d1 = config.delay_for_attempt(1);
        let d2 = config.delay_for_attempt(2);
        let d3 = config.delay_for_attempt(3);

        assert_eq!(d0, Duration::from_secs(1));
        assert_eq!(d1, Duration::from_secs(2));
        assert_eq!(d2, Duration::from_secs(4));
        assert_eq!(d3, Duration::from_secs(8));
    }

    #[test]
    fn test_backoff_delay_capped_at_max() {
        let config = BackoffConfig {
            max_retries: 10,
            initial_delay: Duration::from_secs(1),
            multiplier: 2.0,
            max_delay: Duration::from_secs(5),
            jitter: JitterStrategy::None,
            retryable_statuses: vec![429],
            respect_retry_after: false,
        };

        // Attempt 3 would be 8s uncapped, but max_delay is 5s
        let d = config.delay_for_attempt(3);
        assert_eq!(d, Duration::from_secs(5));

        // Attempt 10 would be 1024s uncapped
        let d = config.delay_for_attempt(10);
        assert_eq!(d, Duration::from_secs(5));
    }

    #[test]
    fn test_backoff_jitter_full_in_range() {
        let config = BackoffConfig {
            max_retries: 3,
            initial_delay: Duration::from_secs(1),
            multiplier: 2.0,
            max_delay: Duration::from_secs(60),
            jitter: JitterStrategy::Full,
            retryable_statuses: vec![429],
            respect_retry_after: false,
        };

        // Full jitter for attempt 0: random in [0, 1s]
        // Run multiple times to verify it's within range
        for _ in 0..100 {
            let d = config.delay_for_attempt(0);
            assert!(d <= Duration::from_secs(1), "delay {:?} > 1s", d);
        }

        // Full jitter for attempt 1: random in [0, 2s]
        for _ in 0..100 {
            let d = config.delay_for_attempt(1);
            assert!(d <= Duration::from_secs(2), "delay {:?} > 2s", d);
        }
    }

    #[test]
    fn test_backoff_none_preset() {
        let config = BackoffConfig::none();
        assert_eq!(config.max_retries, 0);
    }

    #[test]
    fn test_backoff_standard_preset() {
        let config = BackoffConfig::standard();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.initial_delay, Duration::from_secs(1));
        assert_eq!(config.multiplier, 2.0);
        assert!(config.retryable_statuses.contains(&429));
        assert!(config.retryable_statuses.contains(&503));
    }
}
