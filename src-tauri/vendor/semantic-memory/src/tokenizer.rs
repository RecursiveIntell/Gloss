//! Pluggable token counting for context budget management.
//!
//! Provides the [`TokenCounter`] trait for text-to-token-count conversion,
//! with [`EstimateTokenCounter`] as a simple default.

use std::sync::Arc;

/// Trait for counting tokens in text.
///
/// Implement this to plug in tiktoken, sentencepiece, or any
/// model-specific tokenizer for accurate context budget management.
///
/// # Examples
///
/// ```rust
/// use semantic_memory::TokenCounter;
///
/// struct MyTokenizer;
/// impl TokenCounter for MyTokenizer {
///     fn count_tokens(&self, text: &str) -> usize {
///         text.split_whitespace().count()
///     }
/// }
/// ```
pub trait TokenCounter: Send + Sync {
    /// Count the number of tokens in the given text.
    fn count_tokens(&self, text: &str) -> usize;
}

/// Default token counter: estimates tokens as `len / 4`.
///
/// Acceptable for English prose (~4 chars per token on average).
/// Inaccurate for CJK text (~1 token per char), code, or structured data.
/// Replace with a real tokenizer for accurate budget management.
pub struct EstimateTokenCounter;

impl TokenCounter for EstimateTokenCounter {
    fn count_tokens(&self, text: &str) -> usize {
        if text.is_empty() {
            0
        } else {
            (text.len() / 4).max(1)
        }
    }
}

/// Create the default token counter (estimate-based).
pub(crate) fn default_token_counter() -> Arc<dyn TokenCounter> {
    Arc::new(EstimateTokenCounter)
}
