//! Choice extraction from LLM responses.
//!
//! Provides [`parse_choice`] for extracting a single choice from a set of
//! valid options, handling common LLM formatting patterns like bold, quotes,
//! and prose wrapping.

use crate::error::{
    ensure_input_within_limits, record_extracted_span, ParseError, ParseOptions, ParseTrace,
};
use crate::extract::preprocess_opts;

/// Extract a single choice from a set of valid options.
///
/// Handles common LLM response patterns:
/// - Direct match: `"positive"`
/// - Bold: `"**positive**"`
/// - Quoted: `"'positive'"` or `"\"positive\""`
/// - In prose: `"I would classify this as positive because..."`
/// - Parenthesized: `"(positive)"`
///
/// Matching is case-insensitive. If multiple valid choices appear,
/// returns the first one found in the text.
///
/// # Examples
///
/// ```
/// use llm_output_parser::parse_choice;
///
/// let result = parse_choice("I'd classify this as positive", &["positive", "negative"]).unwrap();
/// assert_eq!(result, "positive");
/// ```
pub fn parse_choice<'a>(response: &str, valid_choices: &[&'a str]) -> Result<&'a str, ParseError> {
    let opts = ParseOptions::default();
    let (choice, _trace) = parse_choice_with_trace(response, valid_choices, &opts)?;
    Ok(choice)
}

/// Extract a single choice from a set of valid options with diagnostic trace.
pub fn parse_choice_with_trace<'a>(
    response: &str,
    valid_choices: &[&'a str],
    opts: &ParseOptions,
) -> Result<(&'a str, ParseTrace), ParseError> {
    ensure_input_within_limits(response, opts)?;

    let cleaned = preprocess_opts(response, opts.strip_think_tags);
    let mut trace = ParseTrace::default();

    if cleaned.is_empty() {
        return Err(ParseError::EmptyResponse);
    }

    let lower = cleaned.to_lowercase();

    // Strip common wrappers for exact matching
    let stripped = lower
        .trim_matches(|c: char| c == '.' || c == '!' || c == ',' || c.is_whitespace())
        .trim_start_matches("**")
        .trim_end_matches("**")
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches('(')
        .trim_matches(')')
        .trim();

    trace.strategies_tried.push("exact_match");
    for &choice in valid_choices {
        if stripped.eq_ignore_ascii_case(choice) {
            record_extracted_span(&mut trace, &cleaned, stripped);
            return Ok((choice, trace));
        }
    }

    trace.strategies_tried.push("prefix_match");
    for &choice in valid_choices {
        let choice_lower = choice.to_lowercase();
        if stripped.starts_with(&choice_lower) {
            // Check word boundary after the choice
            let after = stripped.len().min(choice_lower.len());
            if after == stripped.len() || !stripped.as_bytes()[after].is_ascii_alphanumeric() {
                record_extracted_span(&mut trace, &cleaned, choice);
                return Ok((choice, trace));
            }
        }
    }

    trace.strategies_tried.push("word_boundary_search");
    let mut best: Option<(&'a str, usize)> = None;

    for &choice in valid_choices {
        let choice_lower = choice.to_lowercase();
        if let Some(pos) = find_word_boundary_match(&lower, &choice_lower) {
            match best {
                None => best = Some((choice, pos)),
                Some((_, best_pos)) if pos < best_pos => best = Some((choice, pos)),
                _ => {}
            }
        }
    }

    if let Some((choice, pos)) = best {
        trace.extracted_span = Some((pos, pos + choice.len()));
        return Ok((choice, trace));
    }

    Err(ParseError::NoMatchingChoice {
        valid: valid_choices.iter().map(|s| s.to_string()).collect(),
    })
}

/// Find a word-boundary match of `needle` in `haystack`.
/// Returns the position of the first match, or None.
fn find_word_boundary_match(haystack: &str, needle: &str) -> Option<usize> {
    let h_bytes = haystack.as_bytes();
    let n_len = needle.len();
    let mut search_from = 0;

    while let Some(pos) = haystack[search_from..].find(needle) {
        let abs_pos = search_from + pos;
        let end_pos = abs_pos + n_len;

        // Check boundary before
        let boundary_before = abs_pos == 0 || !h_bytes[abs_pos - 1].is_ascii_alphanumeric();

        // Check boundary after
        let boundary_after = end_pos >= haystack.len() || !h_bytes[end_pos].is_ascii_alphanumeric();

        if boundary_before && boundary_after {
            return Some(abs_pos);
        }

        search_from = abs_pos + 1;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match() {
        let result = parse_choice("positive", &["positive", "negative"]).unwrap();
        assert_eq!(result, "positive");
    }

    #[test]
    fn with_period() {
        let result = parse_choice("positive.", &["positive", "negative"]).unwrap();
        assert_eq!(result, "positive");
    }

    #[test]
    fn bold() {
        let result = parse_choice("**positive**", &["positive", "negative"]).unwrap();
        assert_eq!(result, "positive");
    }

    #[test]
    fn quoted() {
        let result = parse_choice("\"positive\"", &["positive", "negative"]).unwrap();
        assert_eq!(result, "positive");
    }

    #[test]
    fn in_prose() {
        let result =
            parse_choice("I'd classify this as positive", &["positive", "negative"]).unwrap();
        assert_eq!(result, "positive");
    }

    #[test]
    fn case_insensitive() {
        let result = parse_choice("POSITIVE", &["positive", "negative"]).unwrap();
        assert_eq!(result, "positive");
    }

    #[test]
    fn first_wins() {
        let result =
            parse_choice("positive and negative aspects", &["positive", "negative"]).unwrap();
        assert_eq!(result, "positive");
    }

    #[test]
    fn with_think() {
        let result = parse_choice("<think>hmm</think>negative", &["positive", "negative"]).unwrap();
        assert_eq!(result, "negative");
    }

    #[test]
    fn no_match() {
        let result = parse_choice("maybe", &["positive", "negative"]);
        assert!(result.is_err());
    }

    #[test]
    fn no_substring() {
        let result = parse_choice("unpositive", &["positive"]);
        assert!(result.is_err());
    }

    #[test]
    fn with_trace_records_strategy() {
        let opts = ParseOptions::default();
        let (choice, trace) =
            parse_choice_with_trace("I would approve this.", &["approve", "reject"], &opts)
                .unwrap();
        assert_eq!(choice, "approve");
        assert!(trace.strategies_tried.contains(&"word_boundary_search"));
        assert!(trace.extracted_span.is_some());
    }

    #[test]
    fn with_trace_rejects_oversized_input() {
        let opts = ParseOptions {
            max_input_bytes: 8,
            ..ParseOptions::default()
        };
        let err = parse_choice_with_trace("definitely approve", &["approve"], &opts).unwrap_err();
        assert_eq!(err.kind(), "too_large");
    }
}
