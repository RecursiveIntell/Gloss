//! Output strategy for configuring how raw LLM text is parsed.
//!
//! [`OutputStrategy`] determines how [`LlmCall::build_output`](crate::llm_call::LlmCall)
//! converts raw LLM text into a `serde_json::Value`. The default (`Lossy`)
//! preserves backward compatibility. Switching to `Json`, `StringList`, or
//! `Custom` enables parse failures, which in turn enables the retry system
//! to detect and correct bad output.

use crate::output_parser::ParseError;
use serde_json::Value;
use std::sync::Arc;

/// Type alias for the custom parse function used in [`OutputStrategy::Custom`].
pub type CustomParseFn = Arc<dyn Fn(&str) -> Result<Value, ParseError> + Send + Sync>;

/// Controls how raw LLM text is parsed into a `serde_json::Value` inside
/// [`LlmCall::build_output`](crate::llm_call::LlmCall).
///
/// The default (`Lossy`) preserves backward compatibility. Switching to
/// `Json`, `StringList`, or `Custom` enables parse failures, which in
/// turn enables the retry system to detect and correct bad output.
///
/// # Example
///
/// ```
/// use llm_pipeline::output_strategy::OutputStrategy;
///
/// let strategy = OutputStrategy::Json;
/// let lossy = OutputStrategy::default();
/// ```
#[derive(Clone)]
pub enum OutputStrategy {
    /// Always succeeds. Tries JSON extraction, falls back to `Value::String`.
    /// This is the current/legacy behavior and the default.
    Lossy,

    /// Uses `output_parser::parse_json_value` — full multi-strategy extraction
    /// with repair. Can fail, producing a parse error in diagnostics.
    Json,

    /// Uses `output_parser::parse_string_list_raw` — extracts a list of strings.
    /// The returned Value is a `Value::Array` of `Value::String`.
    StringList,

    /// Extracts content from a named XML tag via `output_parser::parse_xml_tag`.
    /// The returned Value is a `Value::String` containing the tag body.
    XmlTag(String),

    /// Uses `output_parser::parse_choice` with a set of valid options.
    /// Returns `Value::String` containing the matched choice.
    /// Critical for agent-graph routing nodes.
    Choice(Vec<String>),

    /// Uses `output_parser::parse_number` — extracts a numeric value.
    /// Returns `Value::Number`. Handles "Score: 8.5", "8/10", prose.
    Number,

    /// Uses `output_parser::parse_number_in_range` — bounded numeric extraction.
    /// Returns `Value::Number`. Fails if outside `[min, max]`.
    NumberInRange(f64, f64),

    /// Uses `output_parser::parse_text` — clean text with boilerplate stripping.
    /// Returns `Value::String` with "Sure!", "Here's..." prefixes removed.
    Text,

    /// Caller-provided parse function. Maximum flexibility.
    Custom(CustomParseFn),
}

impl Default for OutputStrategy {
    #[inline]
    fn default() -> Self {
        Self::Lossy
    }
}

impl std::fmt::Debug for OutputStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputStrategy::Lossy => write!(f, "Lossy"),
            OutputStrategy::Json => write!(f, "Json"),
            OutputStrategy::StringList => write!(f, "StringList"),
            OutputStrategy::XmlTag(tag) => write!(f, "XmlTag({:?})", tag),
            OutputStrategy::Choice(choices) => write!(f, "Choice({:?})", choices),
            OutputStrategy::Number => write!(f, "Number"),
            OutputStrategy::NumberInRange(min, max) => {
                write!(f, "NumberInRange({}, {})", min, max)
            }
            OutputStrategy::Text => write!(f, "Text"),
            OutputStrategy::Custom(_) => write!(f, "Custom(...)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_lossy() {
        let strategy = OutputStrategy::default();
        assert!(matches!(strategy, OutputStrategy::Lossy));
    }

    #[test]
    fn test_debug_output() {
        assert_eq!(format!("{:?}", OutputStrategy::Json), "Json");
        assert_eq!(
            format!("{:?}", OutputStrategy::Choice(vec!["a".into(), "b".into()])),
            "Choice([\"a\", \"b\"])"
        );
    }
}
