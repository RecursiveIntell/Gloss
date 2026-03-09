//! Semantic retry with error feedback for LLM output parsing.
//!
//! [`RetryConfig`] enables the LLM-in-the-loop retry system. When the output
//! strategy on [`LlmCall`](crate::llm_call::LlmCall) produces a parse failure,
//! the retry system constructs a correction prompt containing the original
//! request, the bad output, and the error description, then re-calls the model.

use serde_json::Value;
use std::sync::Arc;

/// Type alias for the semantic validator function used in [`RetryConfig`].
pub type ValidatorFn = Arc<dyn Fn(&str, &Value) -> Result<(), String> + Send + Sync>;

/// Configuration for LLM-in-the-loop retry on parse failure.
///
/// When the output strategy on [`LlmCall`](crate::llm_call::LlmCall) produces
/// a parse failure (recorded in [`ParseDiagnostics`](crate::diagnostics::ParseDiagnostics)),
/// and a `RetryConfig` is present, the retry system constructs a correction
/// prompt and re-calls the model.
///
/// # Example
///
/// ```
/// use llm_pipeline::retry::RetryConfig;
///
/// // Simple: retry up to 2 times on parse failure
/// let config = RetryConfig::new(2);
///
/// // With semantic validation
/// let config = RetryConfig::new(2)
///     .requiring_keys(&["title", "year"]);
///
/// // Disable temperature cool-down
/// let config = RetryConfig::new(3).no_cool_down();
/// ```
#[derive(Clone)]
pub struct RetryConfig {
    /// Maximum retry attempts (not counting the initial call). Range: 1-5.
    pub max_retries: u32,

    /// Optional additional validator beyond the OutputStrategy.
    /// If set, this runs AFTER the output strategy succeeds.
    /// Use this for semantic validation (range checks, enum values, etc.)
    /// that goes beyond structural parsing.
    ///
    /// The function receives `(raw_text, parsed_value)` and returns
    /// `Ok(())` on success or `Err(reason)` on failure.
    pub validator: Option<ValidatorFn>,

    /// Lower temperature on each retry. Default: `true`.
    /// Drops by 0.2 per retry (floored at 0.0).
    pub cool_down: bool,
}

impl RetryConfig {
    /// Retry up to N times. Triggers on OutputStrategy parse failure only.
    pub fn new(max_retries: u32) -> Self {
        Self {
            max_retries: max_retries.min(5),
            validator: None,
            cool_down: true,
        }
    }

    /// Retry with an additional semantic validator.
    ///
    /// The validator receives `(raw_text, parsed_value)` and returns
    /// `Ok(())` on success or `Err(reason_string)` on failure.
    pub fn with_validator(
        mut self,
        f: impl Fn(&str, &Value) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.validator = Some(Arc::new(f));
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
}

impl std::fmt::Debug for RetryConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetryConfig")
            .field("max_retries", &self.max_retries)
            .field("has_validator", &self.validator.is_some())
            .field("cool_down", &self.cool_down)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_config_new() {
        let config = RetryConfig::new(3);
        assert_eq!(config.max_retries, 3);
        assert!(config.validator.is_none());
        assert!(config.cool_down);
    }

    #[test]
    fn test_retry_config_capped_at_5() {
        let config = RetryConfig::new(10);
        assert_eq!(config.max_retries, 5);
    }

    #[test]
    fn test_retry_no_cool_down() {
        let config = RetryConfig::new(2).no_cool_down();
        assert!(!config.cool_down);
    }

    #[test]
    fn test_requiring_keys_ok() {
        let config = RetryConfig::new(2).requiring_keys(&["title", "year"]);
        let val = serde_json::json!({"title": "Matrix", "year": 1999});
        let result = config.validator.as_ref().map(|v| v("", &val));
        assert!(result.is_some());
        assert!(result.unwrap().is_ok());
    }

    #[test]
    fn test_requiring_keys_missing() {
        let config = RetryConfig::new(2).requiring_keys(&["title", "year"]);
        let val = serde_json::json!({"title": "Matrix"});
        let result = config.validator.as_ref().map(|v| v("", &val));
        assert!(result.is_some());
        assert!(result.unwrap().is_err());
    }

    #[test]
    fn test_requiring_keys_null() {
        let config = RetryConfig::new(2).requiring_keys(&["title"]);
        let val = serde_json::json!({"title": null});
        let result = config.validator.as_ref().map(|v| v("", &val));
        assert!(result.is_some());
        assert!(result.unwrap().is_err());
    }

    #[test]
    fn test_custom_validator() {
        let config = RetryConfig::new(2).with_validator(|_raw, value| {
            let score = value
                .get("score")
                .and_then(|v| v.as_f64())
                .ok_or("missing score")?;
            if !(0.0..=1.0).contains(&score) {
                return Err(format!("score {} outside 0.0-1.0", score));
            }
            Ok(())
        });

        let good = serde_json::json!({"score": 0.5});
        assert!(config
            .validator
            .as_ref()
            .map(|v| v("", &good))
            .unwrap()
            .is_ok());

        let bad = serde_json::json!({"score": 1.5});
        assert!(config
            .validator
            .as_ref()
            .map(|v| v("", &bad))
            .unwrap()
            .is_err());
    }
}
