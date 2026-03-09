//! Error types and configuration for LLM output parsers.

/// Options controlling parser behavior and safety limits.
///
/// All public parse functions accept `ParseOptions` (via `_with_trace` variants)
/// to configure safety bounds and preprocessing behavior.
///
/// # Defaults
///
/// ```
/// use llm_output_parser::ParseOptions;
///
/// let opts = ParseOptions::default();
/// assert_eq!(opts.max_input_bytes, 2_097_152);
/// assert_eq!(opts.max_nesting_depth, 64);
/// assert_eq!(opts.max_repair_attempts, 3);
/// assert!(opts.strip_think_tags);
/// assert!(opts.allow_code_fences);
/// ```
#[derive(Debug, Clone)]
pub struct ParseOptions {
    /// Maximum input size in bytes. Inputs exceeding this return `ParseError::TooLarge`.
    /// Default: 2,097,152 (2 MB).
    pub max_input_bytes: usize,
    /// Maximum JSON nesting depth tracked during bracket matching.
    /// Inputs exceeding this return `ParseError::TooDeep`.
    /// Default: 64.
    pub max_nesting_depth: usize,
    /// Maximum number of JSON repair attempts before giving up.
    /// Default: 3.
    pub max_repair_attempts: usize,
    /// Whether to strip `<think>` and `<thinking>` blocks during preprocessing.
    /// Default: true.
    pub strip_think_tags: bool,
    /// Whether to attempt extraction from markdown code fences.
    /// Default: true.
    pub allow_code_fences: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            max_input_bytes: 2_097_152,
            max_nesting_depth: 64,
            max_repair_attempts: 3,
            strip_think_tags: true,
            allow_code_fences: true,
        }
    }
}

/// Diagnostic trace capturing the execution path of a parse operation.
///
/// Returned by `_with_trace` variants of parse functions. Records which
/// strategies were attempted, whether repair was applied, and the byte span
/// of the extracted content.
#[derive(Debug, Clone, Default)]
pub struct ParseTrace {
    /// Names of strategies attempted, in order.
    pub strategies_tried: Vec<&'static str>,
    /// Whether JSON repair was applied to produce the final result.
    pub repaired: bool,
    /// Descriptions of repair actions applied (e.g., "remove_trailing_commas").
    pub repair_actions: Vec<String>,
    /// Byte offset span `(start, end)` of the extracted content within the
    /// preprocessed input, if applicable.
    pub extracted_span: Option<(usize, usize)>,
    /// Non-fatal warnings encountered during parsing.
    pub warnings: Vec<String>,
}

/// Errors returned by output parsers.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// The LLM response was empty or whitespace-only.
    #[error("empty LLM response")]
    EmptyResponse,

    /// No parsing strategy could extract the expected format.
    #[error("could not parse {expected_format} from LLM response: {text}")]
    Unparseable {
        /// The format the parser was trying to extract.
        expected_format: &'static str,
        /// A truncated copy of the cleaned LLM text (max 200 chars).
        text: String,
    },

    /// JSON was extracted but failed to deserialize into the target type.
    #[error("JSON deserialization failed: {reason}")]
    DeserializationFailed {
        /// The serde error message.
        reason: String,
        /// The raw JSON string that failed deserialization.
        raw_json: String,
    },

    /// No valid choice from the provided options was found.
    #[error("no valid choice found in response (valid: {valid:?})")]
    NoMatchingChoice {
        /// The list of choices that were searched for.
        valid: Vec<String>,
    },

    /// No number found, or number was outside the expected range.
    #[error("no valid number found in response")]
    NoNumber,

    /// Input exceeds `ParseOptions::max_input_bytes`.
    #[error("input too large: {size} bytes exceeds limit of {limit} bytes")]
    TooLarge {
        /// Actual input size in bytes.
        size: usize,
        /// Configured limit in bytes.
        limit: usize,
    },

    /// JSON nesting exceeds `ParseOptions::max_nesting_depth`.
    #[error("nesting too deep: depth {depth} exceeds limit of {limit}")]
    TooDeep {
        /// Detected nesting depth.
        depth: usize,
        /// Configured limit.
        limit: usize,
    },
}

impl ParseError {
    /// Stable string discriminant for programmatic matching.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::EmptyResponse => "empty_response",
            Self::Unparseable { .. } => "unparseable",
            Self::DeserializationFailed { .. } => "deserialization_failed",
            Self::NoMatchingChoice { .. } => "no_matching_choice",
            Self::NoNumber => "no_number",
            Self::TooLarge { .. } => "too_large",
            Self::TooDeep { .. } => "too_deep",
        }
    }
}

pub(crate) fn ensure_input_within_limits(
    response: &str,
    opts: &ParseOptions,
) -> Result<(), ParseError> {
    if response.len() > opts.max_input_bytes {
        return Err(ParseError::TooLarge {
            size: response.len(),
            limit: opts.max_input_bytes,
        });
    }

    Ok(())
}

pub(crate) fn record_extracted_span(trace: &mut ParseTrace, cleaned: &str, extracted: &str) {
    if extracted.is_empty() {
        return;
    }

    if let Some(start) = cleaned.find(extracted) {
        trace.extracted_span = Some((start, start + extracted.len()));
    }
}

/// Truncate a string to at most `max_len` characters, appending "..." if truncated.
#[allow(dead_code)]
pub(crate) fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}
