//! Typed JSON extraction from LLM responses.
//!
//! Provides [`parse_json`] for extracting typed structs and [`parse_json_value`]
//! for untyped JSON extraction, using a multi-strategy pipeline that handles
//! think blocks, markdown fences, bracket matching, and JSON repair.
//!
//! The `_with_trace` variants return a [`ParseTrace`] alongside the result
//! for observability and debugging.

use serde::de::DeserializeOwned;

use crate::error::{
    ensure_input_within_limits, record_extracted_span, truncate, ParseError, ParseOptions,
    ParseTrace,
};
use crate::extract::{
    extract_code_block, extract_code_block_for, find_bracketed_limited, preprocess_opts,
};
use crate::repair::try_repair_json;

/// Parse an LLM response into a typed struct.
///
/// Strategies (in order):
/// 1. Direct deserialize on preprocessed text
/// 2. Extract from markdown code block (`` ```json ``)
/// 3. Extract from any code block
/// 4. Bracket-match a JSON object (`{...}`)
/// 5. Bracket-match a JSON array (`[...]`)
/// 6. Repair malformed JSON then retry strategies 1-5
///
/// # Errors
///
/// Returns [`ParseError::EmptyResponse`] if input is empty after preprocessing,
/// [`ParseError::DeserializationFailed`] if JSON was found but doesn't match `T`.
///
/// # Examples
///
/// ```
/// use serde::Deserialize;
/// use llm_output_parser::parse_json;
///
/// #[derive(Deserialize, Debug, PartialEq)]
/// struct Analysis {
///     sentiment: String,
///     confidence: f64,
/// }
///
/// let response = r#"<think>analyzing...</think>{"sentiment": "positive", "confidence": 0.92}"#;
/// let result: Analysis = parse_json(response).unwrap();
/// assert_eq!(result.sentiment, "positive");
/// ```
pub fn parse_json<T: DeserializeOwned>(response: &str) -> Result<T, ParseError> {
    let opts = ParseOptions::default();
    let (result, _trace) = parse_json_with_trace::<T>(response, &opts)?;
    Ok(result)
}

/// Parse into a `serde_json::Value` when you don't know the schema.
///
/// Uses the same strategy pipeline as [`parse_json`].
pub fn parse_json_value(response: &str) -> Result<serde_json::Value, ParseError> {
    parse_json(response)
}

/// Parse an LLM response into a typed struct, returning a diagnostic trace.
///
/// Identical to [`parse_json`] but accepts [`ParseOptions`] for safety limits
/// and returns [`ParseTrace`] recording which strategies were attempted.
///
/// # Errors
///
/// Returns [`ParseError::TooLarge`] if input exceeds `opts.max_input_bytes`,
/// [`ParseError::TooDeep`] if JSON nesting exceeds `opts.max_nesting_depth`.
pub fn parse_json_with_trace<T: DeserializeOwned>(
    response: &str,
    opts: &ParseOptions,
) -> Result<(T, ParseTrace), ParseError> {
    ensure_input_within_limits(response, opts)?;

    let mut trace = ParseTrace::default();
    let (candidate, cleaned) = extract_json_candidate_traced(response, opts, &mut trace)?;

    // Try deserializing the candidate
    trace.strategies_tried.push("deserialize_candidate");
    let deser_err = match serde_json::from_str::<T>(&candidate) {
        Ok(val) => return Ok((val, trace)),
        Err(e) => e.to_string(),
    };

    // Try repair on the candidate (bounded)
    let mut repair_attempts = 0;
    if repair_attempts < opts.max_repair_attempts {
        trace.strategies_tried.push("repair_candidate");
        repair_attempts += 1;
        if let Some(repaired) = try_repair_json(&candidate) {
            trace.repaired = true;
            trace.repair_actions.push("repaired_candidate".to_string());
            if let Ok(val) = serde_json::from_str::<T>(&repaired) {
                return Ok((val, trace));
            }
        }
    }

    // Try repair on the full cleaned text if different from candidate
    if candidate != cleaned && repair_attempts < opts.max_repair_attempts {
        trace.strategies_tried.push("repair_cleaned");
        repair_attempts += 1;
        if let Some(repaired) = try_repair_json(&cleaned) {
            trace.repaired = true;
            trace.repair_actions.push("repaired_cleaned".to_string());
            if let Ok(val) = serde_json::from_str::<T>(&repaired) {
                return Ok((val, trace));
            }
        }
    }

    // Suppress unused-variable warning for future use
    let _ = repair_attempts;

    // All strategies exhausted
    Err(ParseError::DeserializationFailed {
        reason: deser_err,
        raw_json: truncate(&candidate, 200),
    })
}

/// Parse into a `serde_json::Value` with diagnostic trace.
///
/// See [`parse_json_with_trace`] for details.
pub fn parse_json_value_with_trace(
    response: &str,
    opts: &ParseOptions,
) -> Result<(serde_json::Value, ParseTrace), ParseError> {
    parse_json_with_trace(response, opts)
}

/// Traced version of candidate extraction with safety limits.
fn extract_json_candidate_traced(
    response: &str,
    opts: &ParseOptions,
    trace: &mut ParseTrace,
) -> Result<(String, String), ParseError> {
    let trimmed = response.trim();

    if trimmed.is_empty() {
        return Err(ParseError::EmptyResponse);
    }

    let cleaned = preprocess_opts(trimmed, opts.strip_think_tags);

    if cleaned.is_empty() {
        return Err(ParseError::EmptyResponse);
    }

    // Strategy 1: Direct parse on cleaned text
    trace.strategies_tried.push("direct_parse");
    if serde_json::from_str::<serde_json::Value>(&cleaned).is_ok() {
        trace.extracted_span = Some((0, cleaned.len()));
        return Ok((cleaned.clone(), cleaned));
    }

    if opts.allow_code_fences {
        // Strategy 2: Extract from ```json code block
        trace.strategies_tried.push("json_code_block");
        if let Some(content) = extract_code_block_for(&cleaned, "json") {
            record_extracted_span(trace, &cleaned, content);
            if serde_json::from_str::<serde_json::Value>(content).is_ok() {
                return Ok((content.to_string(), cleaned));
            }
            // Even if not valid yet, this is a good candidate for repair
            return Ok((content.to_string(), cleaned));
        }

        // Strategy 3: Extract from any code block
        trace.strategies_tried.push("any_code_block");
        if let Some((_lang, content)) = extract_code_block(&cleaned) {
            // Check if it looks like JSON (starts with { or [)
            let trimmed_content = content.trim();
            if trimmed_content.starts_with('{') || trimmed_content.starts_with('[') {
                record_extracted_span(trace, &cleaned, trimmed_content);
                if serde_json::from_str::<serde_json::Value>(trimmed_content).is_ok() {
                    return Ok((trimmed_content.to_string(), cleaned));
                }
                return Ok((trimmed_content.to_string(), cleaned));
            }
        }
    }

    // Strategy 4: Bracket-match a JSON object (with depth limit)
    trace.strategies_tried.push("bracket_match_object");
    if let Some(bracket_str) = find_bracketed_limited(&cleaned, '{', '}', opts.max_nesting_depth)? {
        record_extracted_span(trace, &cleaned, bracket_str);
        if serde_json::from_str::<serde_json::Value>(bracket_str).is_ok() {
            return Ok((bracket_str.to_string(), cleaned));
        }
        return Ok((bracket_str.to_string(), cleaned));
    }

    // Strategy 5: Bracket-match a JSON array (with depth limit)
    trace.strategies_tried.push("bracket_match_array");
    if let Some(bracket_str) = find_bracketed_limited(&cleaned, '[', ']', opts.max_nesting_depth)? {
        record_extracted_span(trace, &cleaned, bracket_str);
        if serde_json::from_str::<serde_json::Value>(bracket_str).is_ok() {
            return Ok((bracket_str.to_string(), cleaned));
        }
        return Ok((bracket_str.to_string(), cleaned));
    }

    // No candidate found — return cleaned text as the candidate for repair
    Ok((cleaned.clone(), cleaned))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Kv {
        key: String,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Sentiment {
        sentiment: String,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Outer {
        outer: Inner,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Inner {
        inner: Vec<i32>,
    }

    #[test]
    fn direct_json_object() {
        let input = r#"{"key": "value"}"#;
        let result: Kv = parse_json(input).unwrap();
        assert_eq!(result.key, "value");
    }

    #[test]
    fn direct_json_array() {
        let input = "[1, 2, 3]";
        let result: Vec<i32> = parse_json(input).unwrap();
        assert_eq!(result, vec![1, 2, 3]);
    }

    #[test]
    fn think_then_json() {
        let input = r#"<think>analyzing</think>{"key": "value"}"#;
        let result: Kv = parse_json(input).unwrap();
        assert_eq!(result.key, "value");
    }

    #[test]
    fn code_block_json() {
        let input = "Here's the data:\n```json\n{\"key\": \"value\"}\n```";
        let result: Kv = parse_json(input).unwrap();
        assert_eq!(result.key, "value");
    }

    #[test]
    fn bare_code_block() {
        let input = "```\n{\"key\": \"value\"}\n```";
        let result: Kv = parse_json(input).unwrap();
        assert_eq!(result.key, "value");
    }

    #[test]
    fn json_in_prose() {
        let input = r#"The analysis is {"sentiment": "positive"} as shown."#;
        let result: Sentiment = parse_json(input).unwrap();
        assert_eq!(result.sentiment, "positive");
    }

    #[test]
    fn nested_json() {
        let input = r#"{"outer": {"inner": [1,2,3]}}"#;
        let result: Outer = parse_json(input).unwrap();
        assert_eq!(result.outer.inner, vec![1, 2, 3]);
    }

    #[test]
    fn repaired_trailing_comma() {
        let input = r#"{"key": "value",}"#;
        let result: Kv = parse_json(input).unwrap();
        assert_eq!(result.key, "value");
    }

    #[test]
    fn repaired_single_quotes() {
        let input = "{'key': 'value'}";
        let result: Kv = parse_json(input).unwrap();
        assert_eq!(result.key, "value");
    }

    #[test]
    fn think_and_code_block() {
        let input = "<think>hmm</think>\n```json\n{\"key\": \"value\"}\n```";
        let result: Kv = parse_json(input).unwrap();
        assert_eq!(result.key, "value");
    }

    #[test]
    fn json_with_surrounding_text() {
        let input = "Sure! Here's your result: {\"key\": \"value\"}\nHope that helps!";
        let result: Kv = parse_json(input).unwrap();
        assert_eq!(result.key, "value");
    }

    #[test]
    fn parse_json_value_works() {
        let input = r#"{"a": 1, "b": "two"}"#;
        let val = parse_json_value(input).unwrap();
        assert_eq!(val["a"], 1);
        assert_eq!(val["b"], "two");
    }

    #[test]
    fn empty_response_fails() {
        let result: Result<Kv, _> = parse_json("");
        assert!(result.is_err());
    }

    // -- Traced variant tests --

    #[test]
    fn parse_json_value_with_trace_direct() {
        let input = r#"{"a": 1}"#;
        let opts = ParseOptions::default();
        let (val, trace) = parse_json_value_with_trace(input, &opts).unwrap();
        assert_eq!(val["a"], 1);
        assert!(trace.strategies_tried.contains(&"direct_parse"));
        assert!(!trace.repaired);
    }

    #[test]
    fn parse_json_with_trace_repair() {
        let input = "{'key': 'value'}";
        let opts = ParseOptions::default();
        let (val, trace): (Kv, ParseTrace) = parse_json_with_trace(input, &opts).unwrap();
        assert_eq!(val.key, "value");
        assert!(trace.repaired);
        assert!(!trace.repair_actions.is_empty());
    }

    #[test]
    fn parse_json_with_trace_code_block() {
        let input = "Here:\n```json\n{\"key\": \"value\"}\n```";
        let opts = ParseOptions::default();
        let (val, trace): (Kv, ParseTrace) = parse_json_with_trace(input, &opts).unwrap();
        assert_eq!(val.key, "value");
        assert!(trace.strategies_tried.contains(&"json_code_block"));
    }

    // -- Safety limit tests --

    #[test]
    fn too_large_input() {
        let input = "x".repeat(100);
        let opts = ParseOptions {
            max_input_bytes: 50,
            ..Default::default()
        };
        let result = parse_json_value_with_trace(&input, &opts);
        match result {
            Err(ParseError::TooLarge {
                size: 100,
                limit: 50,
            }) => {}
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[test]
    fn too_large_exact_boundary() {
        // Input at exactly the limit should succeed (not TooLarge)
        let input = r#"{"a":1}"#;
        let opts = ParseOptions {
            max_input_bytes: input.len(),
            ..Default::default()
        };
        let result = parse_json_value_with_trace(input, &opts);
        assert!(result.is_ok());
    }

    #[test]
    fn too_deep_nesting() {
        // Wrap deeply nested JSON in prose so direct serde parse fails
        // and bracket matching runs (which enforces depth limits)
        let mut nested = String::new();
        for _ in 0..5 {
            nested.push_str(r#"{"a":"#);
        }
        nested.push('1');
        for _ in 0..5 {
            nested.push('}');
        }
        let input = format!("Here is the result: {nested}");
        let opts = ParseOptions {
            max_nesting_depth: 3,
            ..Default::default()
        };
        let result = parse_json_value_with_trace(&input, &opts);
        match result {
            Err(ParseError::TooDeep { .. }) => {}
            other => panic!("expected TooDeep, got {other:?}"),
        }
    }

    #[test]
    fn too_deep_exact_boundary() {
        // Wrap in prose to force bracket matching; depth exactly at limit should succeed
        let input = r#"Here: {"a": {"b": 1}}"#; // depth 2
        let opts = ParseOptions {
            max_nesting_depth: 2,
            ..Default::default()
        };
        let result = parse_json_value_with_trace(input, &opts);
        assert!(result.is_ok());
    }

    #[test]
    fn strip_think_tags_option_disabled() {
        let input = r#"<think>{"key": "wrong"}</think>{"key": "right"}"#;
        let opts = ParseOptions {
            strip_think_tags: false,
            ..Default::default()
        };
        // With stripping disabled, the think tags stay and the first JSON wins
        let (val, _trace) = parse_json_value_with_trace(input, &opts).unwrap();
        // The bracket matcher will find the last {}, which is {"key": "right"}
        assert_eq!(val["key"], "right");
    }

    #[test]
    fn code_fences_option_disabled() {
        // With code fences disabled, should still find JSON via bracket matching
        let input = "```json\n{\"key\": \"fenced\"}\n```\n{\"key\": \"bare\"}";
        let opts = ParseOptions {
            allow_code_fences: false,
            ..Default::default()
        };
        let (val, trace): (Kv, ParseTrace) = parse_json_with_trace(input, &opts).unwrap();
        // Should NOT have tried code block strategies
        assert!(!trace.strategies_tried.contains(&"json_code_block"));
        assert!(!trace.strategies_tried.contains(&"any_code_block"));
        assert_eq!(val.key, "bare");
    }
}
